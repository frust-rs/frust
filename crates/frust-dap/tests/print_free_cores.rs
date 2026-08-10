//! Print-free-cores conformance scan (wire-corruption tripwire).
//!
//! Regression protection: in stdio mode, `frust-dap`'s `stdout` *is* the DAP
//! wire — a `Content-Length`-framed JSON message stream to the client. A
//! stray `println!`/`print!` anywhere in this crate corrupts that stream;
//! `eprintln!`/`eprint!` corrupt a client's captured stderr diagnostics
//! instead. Every diagnostic goes through `log` (see `crates/frust-dap/src/lib.rs`'s
//! crate-level doc comment).
//!
//! Zero allowlist, unlike `frust-drive`'s (which carves out its CLI-entry
//! points) — `frust-dap` has no CLI entry of its own to carve out; every
//! module in `frust-dap/src` must be print-free. Mirrors `frust-mcp/tests/print_free_cores.rs`'s
//! zero-allowlist shape, which mirrors `frust-drive`'s — keep the scan
//! machinery in the three tripwires consistent as a family.
//!
//! This is a plain `std::fs` source scan run as an ordinary `cargo test`.

use std::fs;
use std::path::{Path, PathBuf};

/// `frust-dap/src`, resolved from this crate's manifest dir.
fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// `path` relative to `src/`, forward-slashed, for stable failure messages.
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

/// The print macros that corrupt the stdio DAP wire (or a client's captured
/// stderr diagnostics). `println!`/`eprintln!` are caught by `"println!"`
/// (eprintln! ends in it); `print!`/`eprint!` by `"print!"`.
const PRINT_NEEDLES: &[&str] = &["println!", "print!"];

/// True if `line` contains any [`PRINT_NEEDLES`] token.
fn has_print(line: &str) -> bool {
    PRINT_NEEDLES.iter().any(|needle| line.contains(needle))
}

/// The name of the free function `line_idx` (0-based) sits in, or `None`.
/// Scans backward for the nearest line whose trimmed text begins a `fn` item.
fn enclosing_fn(lines: &[&str], line_idx: usize) -> Option<String> {
    for raw in lines[..=line_idx].iter().rev() {
        let trimmed = raw.trim_start();
        // Skip comment lines.
        if trimmed.starts_with("//") {
            continue;
        }
        if let Some(name) = fn_name(trimmed) {
            return Some(name);
        }
    }
    None
}

/// Extract the function name from a trimmed line that declares a `fn`, or `None`.
fn fn_name(trimmed: &str) -> Option<String> {
    let idx = find_fn_keyword(trimmed)?;
    let after = &trimmed[idx + 3..];
    let name: String = after
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Index of the `fn ` keyword in `trimmed`, or `None`.
fn find_fn_keyword(trimmed: &str) -> Option<usize> {
    if trimmed.starts_with("fn ") {
        return Some(0);
    }
    trimmed.find(" fn ").map(|i| i + 1)
}

/// A file's production lines (0-based), dropping comment-only lines and
/// everything from `#[cfg(test)]`/`mod tests` onward.
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
fn dap_server_is_print_free_for_stdio_wire_safety() {
    let mut failures = Vec::new();

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
            failures.push(format!(
                "{relp}:{}: `println!`/`print!` inside fn `{func}` — frust-dap's stdout is the \
                 stdio DAP wire and must log via `log::info!`/`log::warn!`/etc. instead. Line: {}",
                i + 1,
                lines[i].trim()
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "print-free-cores ban violated ({} hit(s)) — see crates/frust-dap/src/lib.rs's \
         crate-level doc comment:\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

/// Guards the scanner: `fn_name` and `enclosing_fn` resolve correctly.
#[test]
fn scanner_resolves_enclosing_fn_for_this_codebases_shapes() {
    assert_eq!(fn_name("fn run() -> Result<u8> {").as_deref(), Some("run"));
    assert_eq!(
        fn_name("pub async fn run_session(").as_deref(),
        Some("run_session")
    );
    assert_eq!(
        fn_name("pub(crate) fn tail_lines(s: &str) {").as_deref(),
        Some("tail_lines")
    );
    assert_eq!(fn_name("    let fnord = 1;"), None);

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
