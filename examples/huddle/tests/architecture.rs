//! Architecture conformance test (huddle clean-architecture refactor, task 06
//! — `workflow/plans/features/huddle-clean-architecture/`).
//!
//! A plain `std::fs` source scan over `src/`, run as an ordinary `cargo test`
//! (this repo has no lint-plugin/static-analysis tooling — see
//! `docs/CODE_STANDARDS.md`), enforcing the per-feature layering rules PLAN's
//! Design Decisions 2/3/7 established:
//!
//! (a) a `features/*/domain/**` file never mentions `frust::`, `::presentation::`,
//!     or `::data::` (which subsumes `crate::data::store`) — domain is the
//!     framework- and layer-free core;
//! (b) a `features/*/presentation/**` file never mentions `::data::` — a
//!     presentation file reaches the shared dataset through an injected
//!     repository trait, never the store or a sibling feature's `data/`
//!     module, directly;
//! (c) only a `*/data/**` file (or `src/data/` itself) mentions
//!     `crate::data::store` — the raw shared dataset is the data layer's to
//!     read;
//! (d) no file anywhere mentions `crate::mock` or `crate::screens` — both
//!     modules are deleted by this task, so any surviving reference is stale;
//! (e) `features::search`/`features::profile`'s `domain`+`data` never mention
//!     `HuddleFailure`, `ControllerCore`, or `async fn` — the Design
//!     Decision 8 ratchet keeping both features' sync-infallible shape from
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
//! the real `Store<Feature>Repository` under `#[cfg(test)]` (task 02's
//! documented mini-composition-root pattern, task 04/05's precedent) so
//! their assertions can target the real dataset shape without a test-logic
//! rewrite (PLAN Design Decision 5's hard behavior-preserving bar). Doc
//! comments are stripped for the same reason tasks 02-05 flagged repeatedly:
//! several domain/data files carry historical or intra-doc-link prose
//! mentioning `crate::mock`/`crate::data::store`/`frust::` (e.g.
//! `profile/domain/entities.rs`'s task-01-inherited link) that names, not
//! imports, the thing it's talking about.
//!
//! # Sanctioned exemptions (explicit, file-scoped, comment-documented)
//!
//! Beyond the `#[cfg(test)]`/doc-comment stripping above, two further
//! documented exceptions exist, each an explicit `(file, needle)` pair below
//! rather than a blanket loosening of the ban it exempts from — see each
//! test function's own doc comment for the full rationale:
//!
//! 1. `features/messages/presentation/controllers.rs`'s `resolve_repo()`
//!    fallback (task 03 ripple) — see
//!    [`presentation_never_imports_data_directly`].
//! 2. `features/settings/domain/{models.rs, accent.rs,
//!    use_cases/set_theme.rs}`'s `frust::` value-type/side-effect imports
//!    (task 05 ripple) — see [`domain_never_imports_frust_presentation_or_data`].
//!
//! Every exemption below is also asserted **used** (fired at least once) —
//! an exemption nobody's code needs any more is exactly as stale as a
//! violation, and should be deleted along with whatever code prompted it.

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
/// `rustc`'s own numbering) plus its raw text.
struct Line {
    number: usize,
    text: String,
}

/// A file's production-code lines: every doc-comment (`//!`/`///`) or plain
/// `//` line-comment-only line dropped, and everything from the file's
/// `#[cfg(test)]`/`mod tests` marker onward truncated (huddle convention
/// keeps the test module last-in-file — verified true for every file in this
/// crate that currently has one, per task 06's completion summary audit).
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
    out
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
/// `crate::data::store` directly (check c).
fn is_data_layer_file(relp: &str) -> bool {
    relp.starts_with("data/") || relp.contains("/data/")
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
// (a) domain never mentions frust::, ::presentation::, ::data:: (which
//     subsumes crate::data::store)
// ---------------------------------------------------------------------------

/// PLAN's third exception category (task 05 ripple): settings is the one
/// feature whose domain use cases ARE theming, so three of its files import
/// `frust::` value types (`Theme`/`Color`/`ColorScheme`/`Brightness`/etc,
/// composed by `compose()`) plus `use_cases/set_theme.rs`'s
/// `set_app_theme`/`clear_app_theme` calls — the app's single theming
/// side-effect site (pre-existing behavior, preserved verbatim per PLAN
/// Design Decision 5). This exemption covers ONLY the `frust::` needle for
/// exactly these three files — every other domain ban (`::presentation::`,
/// `::data::`) still applies to settings' domain like every other feature's
/// (asserted below: the loop applies all three needles uniformly, and only
/// the `frust::` needle consults this table).
#[test]
fn domain_never_imports_frust_presentation_or_data() {
    let frust_exemptions = [
        Exemption {
            file: "features/settings/domain/models.rs",
            needle: "frust::",
            reason: "compose()'s Theme/Color/ColorScheme/Brightness/DesignLanguage/TypeScale \
                  value-type imports — theming IS this domain's subject matter (task 05 \
                  completion summary, Notable Decisions #1)",
        },
        Exemption {
            file: "features/settings/domain/accent.rs",
            needle: "frust::",
            reason: "Brightness/Color/ColorScheme/Theme value-type imports, same rationale as \
                  models.rs above (task 05 completion summary, Notable Decisions #1)",
        },
        Exemption {
            file: "features/settings/domain/use_cases/set_theme.rs",
            needle: "frust::",
            reason: "set_app_theme/clear_app_theme — the app's single theming side-effect \
                  call site, the effect this use case exists to apply (task 05 completion \
                  summary, Notable Decisions #1)",
        },
    ];
    let mut used = vec![false; frust_exemptions.len()];

    let mut failures = Vec::new();
    for path in domain_files() {
        let relp = rel(&path);
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        for line in production_lines(&contents) {
            for needle in ["frust::", "::presentation::", "::data::"] {
                if !line.text.contains(needle) {
                    continue;
                }
                if needle == "frust::"
                    && let Some(idx) = frust_exemptions
                        .iter()
                        .position(|e| e.file == relp && line.text.contains(e.needle))
                {
                    used[idx] = true;
                    continue;
                }
                failures.push(format!(
                    "{relp}:{}: domain file mentions banned `{needle}` — {}",
                    line.number,
                    line.text.trim()
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "domain layering ban violated ({} file:line hit(s)) — a domain file must not mention \
         `frust::` (except the settings allowlist above), `::presentation::`, or `::data::`:\n{}",
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
// (b) presentation never mentions ::data::
// ---------------------------------------------------------------------------

/// Sanctioned exemption (task 03 ripple, PLAN's second exception category):
/// `MessagesController::resolve_repo()`'s `StoreMessageRepository` fallback
/// is a documented composition-root reference in production code — the ONE
/// presentation->data edge in the whole crate that isn't `#[cfg(test)]`-gated
/// (unlike task 02's channels precedent). It exists because
/// `new`/`with_latency`/`for_channel` are public constructors external
/// integration-test crates call with a channel id only, so a repo parameter
/// would force non-import-line test edits (forbidden by Design Decision 5);
/// the injected and fallback repos are the identical `StoreMessageRepository`
/// type, so behavior is unchanged either way (task 03 completion summary,
/// Notable Decisions #1).
#[test]
fn presentation_never_imports_data_directly() {
    let exemptions = [Exemption {
        file: "features/messages/presentation/controllers.rs",
        needle: "::data::",
        reason: "resolve_repo()'s StoreMessageRepository fallback — documented \
                  composition-root reference kept because new/with_latency/for_channel \
                  must stay signature-stable for external integration-test crates \
                  (task 03 completion summary, Notable Decisions #1)",
    }];
    let mut used = vec![false; exemptions.len()];

    let mut failures = Vec::new();
    for path in presentation_files() {
        let relp = rel(&path);
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        for line in production_lines(&contents) {
            if !line.text.contains("::data::") {
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
         not mention `::data::` outside the resolve_repo allowlist above:\n{}",
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

/// Unlike (a)-(c), this scans **every line, including comments and test
/// regions** — `src/mock/` and `src/screens/` no longer exist anywhere in
/// this crate (task 01/05/06), so there is no legitimate reason for even a
/// doc-comment prose mention of either path to survive; any hit means a
/// stale reference this task's own deletion should have caught.
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
// (e) search/profile domain+data never mention HuddleFailure, ControllerCore,
//     or async fn (Design Decision 8 ratchet)
// ---------------------------------------------------------------------------

/// PLAN Design Decision 8: `search` and `profile` are the two
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
