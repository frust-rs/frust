//! Print-free-cores conformance scan (tty-garbling fix).
//!
//! Regression protection for the bug where a `frust tui` session (or build)
//! garbled the whole terminal: a drive pipeline reachable from the TUI
//! `println!`-ed its child's output (an `xcodebuild` env dump, gradle lines)
//! straight to the parent's stdout. While the TUI holds that terminal in raw
//! mode, those bytes bypass ratatui and spray a diagonal LF-without-CR
//! staircase across the screen.
//!
//! The seam boundary the fix established (see `docs/ARCHITECTURE.md`'s Module
//! Structure — the `frust-drive` row — and CLI flow): **a drive core emits
//! only through its `on_line`/stream callback; the CLI *front-end* is the one
//! layer allowed to `println!`.** So every drive module must be print-free
//! EXCEPT the three CLI-invoked one-shot entry functions
//! (`android_run::run`, `ios_run::run`, `ios_run::run_physical`), which
//! legitimately print because the CLI drives them directly and wants their
//! output on stdout.
//!
//! This is a plain `std::fs` source scan run as an ordinary `cargo test` (this
//! repo has no lint-plugin tooling — see `docs/CODE_STANDARDS.md`; precedent:
//! `examples/huddle/tests/architecture.rs`). For every `.rs` under
//! `frust-drive/src`, it finds each `println!`/`eprintln!`/`print!`/`eprint!`
//! in production code (test modules and comment-only lines stripped), maps it
//! to its enclosing free function, and asserts that function is on the CLI
//! allowlist. A new print anywhere in a drive core — the exact regression the
//! fix closed — is an un-allowlisted hit and fails.
//!
//! # What this is NOT
//!
//! A substring/line scan, not a parser. Enclosing-function resolution is a
//! nearest-preceding-`fn`-line backward scan (see [`enclosing_fn`]), which is
//! correct for this codebase because every print sits directly in a free
//! function whose signature is the nearest preceding `fn` line — drive code
//! never nests a print inside an inner `fn` item. A future print buried in a
//! nested `fn` item could be misattributed; no such shape exists today.

use std::fs;
use std::path::{Path, PathBuf};

/// `frust-drive/src`, resolved from this crate's manifest dir so the scan is
/// working-directory-independent.
fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// `path` relative to `src/`, forward-slashed, for stable failure messages and
/// allowlist keys independent of host path separators.
fn rel(path: &Path) -> String {
    path.strip_prefix(src_dir())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display())) {
        let path = entry.unwrap_or_else(|e| panic!("dir entry: {e}")).path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The print macros a raw-mode terminal cannot tolerate a drive core emitting.
/// `println!`/`eprintln!` are caught by the `"println!"` needle (an
/// `eprintln!` ends in `println!`); `print!`/`eprint!` by `"print!"`.
const PRINT_NEEDLES: &[&str] = &["println!", "print!"];

/// True if `line` contains any [`PRINT_NEEDLES`] token.
fn has_print(line: &str) -> bool {
    PRINT_NEEDLES.iter().any(|needle| line.contains(needle))
}

/// `(file, fn)` pairs where a `println!` is sanctioned: the CLI-invoked
/// one-shot entry functions. These print because the CLI drives them directly
/// and wants their output on stdout (see `docs/ARCHITECTURE.md`'s CLI flow);
/// their *session-core* counterparts (`prepare_session`/`spawn_session`/
/// `prepare_simulator_session`/`spawn_physical_session`/…) in the same files
/// are print-free and must stay so, which is exactly what this scan pins.
/// (`android_run`'s CLI path is `run` → the testable `run_with_env`, where the
/// prints live; `ios_run`'s `run`/`run_physical` print directly.)
const CLI_ENTRY_ALLOWLIST: &[(&str, &str)] = &[
    ("android_run/mod.rs", "run_with_env"),
    ("ios_run/mod.rs", "run"),
    ("ios_run/mod.rs", "run_physical"),
];

/// The name of the free function `line_idx` (0-based) sits in, by scanning
/// backward for the nearest line whose trimmed text begins a `fn` item
/// (`fn NAME`, or any `pub`/`pub(...)`/`async`/`const`/`unsafe`-qualified form
/// ending in `fn NAME`). Returns `None` if none precedes it (module-scope,
/// which no print in this codebase occupies).
fn enclosing_fn(lines: &[&str], line_idx: usize) -> Option<String> {
    for raw in lines[..=line_idx].iter().rev() {
        let trimmed = raw.trim_start();
        // Skip comment lines so a prose mention of ` fn ` in a doc/line comment
        // is never mistaken for the enclosing declaration.
        if trimmed.starts_with("//") {
            continue;
        }
        if let Some(name) = fn_name(trimmed) {
            return Some(name);
        }
    }
    None
}

/// Extract the function name from a trimmed line that declares a `fn`, or
/// `None`. Recognizes the qualifier soup before `fn ` (`pub`, `pub(crate)`,
/// `async`, `const`, `unsafe`, `extern "C"`) by finding the `fn ` token and
/// reading the identifier after it.
fn fn_name(trimmed: &str) -> Option<String> {
    // Only treat a line as a declaration if `fn ` appears with a keyword/start
    // boundary before it — avoids matching e.g. a `fn` substring mid-identifier
    // (there are none in practice, but keep the rule explicit).
    let idx = find_fn_keyword(trimmed)?;
    let after = &trimmed[idx + 3..];
    let name: String = after
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Index of the `fn ` keyword in `trimmed`, requiring it to be at the start or
/// preceded by whitespace (a word boundary), or `None`.
fn find_fn_keyword(trimmed: &str) -> Option<usize> {
    if trimmed.starts_with("fn ") {
        return Some(0);
    }
    trimmed.find(" fn ").map(|i| i + 1)
}

/// A file's production lines as `(0-based index, raw text)`, dropping
/// comment-only lines and everything from the file's `#[cfg(test)]`/`mod tests`
/// marker onward (drive convention keeps the test module last-in-file). Raw
/// line indices are preserved so [`enclosing_fn`] can backward-scan the whole
/// file; the returned indices point into the ORIGINAL `lines` slice.
fn production_indices(lines: &[&str]) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, raw) in lines.iter().enumerate() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("mod tests") {
            break;
        }
        if trimmed.starts_with("//") {
            continue;
        }
        out.push(i);
    }
    out
}

#[test]
fn drive_cores_are_print_free_outside_the_cli_entry_allowlist() {
    let mut failures = Vec::new();
    let mut used = vec![false; CLI_ENTRY_ALLOWLIST.len()];

    for path in rust_files(&src_dir()) {
        let relp = rel(&path);
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        let lines: Vec<&str> = contents.lines().collect();

        for i in production_indices(&lines) {
            if !has_print(lines[i]) {
                continue;
            }
            let func = enclosing_fn(&lines, i).unwrap_or_else(|| "<module scope>".to_string());
            if let Some(idx) = CLI_ENTRY_ALLOWLIST
                .iter()
                .position(|(f, fname)| *f == relp && *fname == func)
            {
                used[idx] = true;
                continue;
            }
            failures.push(format!(
                "{relp}:{}: `println!`/`print!` inside fn `{func}` — a drive core must route \
                 output through its `on_line`/stream callback, not print to the tty (only the \
                 CLI-entry allowlist may print). Line: {}",
                i + 1,
                lines[i].trim()
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "print-free-cores ban violated ({} hit(s)) — see docs/ARCHITECTURE.md's CLI flow / \
         the tty-garbling fix:\n{}",
        failures.len(),
        failures.join("\n"),
    );

    // Every allowlist entry must actually fire — a stale entry (a CLI-entry fn
    // that stopped printing, or was renamed) is as much drift as an unlisted
    // violation and should be removed from CLI_ENTRY_ALLOWLIST.
    let stale: Vec<String> = CLI_ENTRY_ALLOWLIST
        .iter()
        .zip(&used)
        .filter(|(_, u)| !**u)
        .map(|((f, fname), _)| format!("  {f}::{fname}"))
        .collect();
    assert!(
        stale.is_empty(),
        "{} CLI-entry allowlist entry(ies) never matched a print — remove the stale entry(ies) \
         from tests/print_free_cores.rs:\n{}",
        stale.len(),
        stale.join("\n"),
    );
}

/// Guards the scanner itself: `fn_name` and `enclosing_fn` must resolve the
/// enclosing free function for the shapes this codebase uses, so a real
/// violation can't slip through a scanner that quietly resolves nothing.
#[test]
fn scanner_resolves_enclosing_fn_for_this_codebases_shapes() {
    assert_eq!(fn_name("fn run() -> Result<u8> {").as_deref(), Some("run"));
    assert_eq!(
        fn_name("pub fn spawn_session(").as_deref(),
        Some("spawn_session")
    );
    assert_eq!(
        fn_name("pub(crate) fn tail_lines(s: &str) {").as_deref(),
        Some("tail_lines")
    );
    assert_eq!(fn_name("    let fnord = 1;"), None);
    assert_eq!(fn_name("// fn commented"), Some("commented".to_string()));

    let lines = vec![
        "fn outer() {",
        "    println!(\"hi\");",
        "}",
        "pub fn inner_entry() {",
        "    println!(\"bye\");",
        "}",
    ];
    assert_eq!(enclosing_fn(&lines, 1).as_deref(), Some("outer"));
    assert_eq!(enclosing_fn(&lines, 4).as_deref(), Some("inner_entry"));
}
