//! Source-scan guard for the wrapper-view swap-blind shape.
//!
//! A view declaring `type Element = Box<dyn Widget>` while holding an
//! `AnyView<..>` field hides inner type swaps from the reconciler's focus
//! bookkeeping (LIMITATIONS id `focus-wrapper-erasure-swap-blind`). There is no
//! type-system fix, so this test walks the in-tree view sources and fails when
//! a new view takes that shape. The scan is deliberately conservative: it may
//! miss exotic formattings (false negatives) but must not flag other shapes.

use std::fs;
use std::path::{Path, PathBuf};

const LIMITATIONS_ID: &str = "focus-wrapper-erasure-swap-blind";
const ALLOW_LIST: &[&str] = &["ReorderableListView"];
const ERASED_ELEMENT: &str = "type Element = Box<dyn Widget>";

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Drop comment lines and collapse all whitespace runs to single spaces.
fn normalise(src: &str) -> String {
    let code: Vec<&str> = src
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect();
    code.join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Text of the `{ .. }` block whose opening brace is at `open`.
fn block_from(text: &str, open: usize) -> &str {
    let mut depth = 0usize;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &text[open..open + i + 1];
                }
            }
            _ => {}
        }
    }
    &text[open..]
}

fn ident_after(s: &str) -> String {
    s.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// Names of every `impl .. View<..> for Name` block whose body declares the
/// erased element type.
fn erased_views(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for (start, _) in text.match_indices("impl") {
        let before_ok = text[..start]
            .chars()
            .last()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        let after = &text[start + 4..];
        if !before_ok || !(after.starts_with('<') || after.starts_with(' ')) {
            continue;
        }
        let Some(brace) = after.find('{') else {
            continue;
        };
        let header = &after[..brace];
        if !header.contains("View<") {
            continue;
        }
        let Some(for_at) = header.find(" for ") else {
            continue;
        };
        let name = ident_after(&header[for_at + 5..]);
        if name.is_empty() {
            continue;
        }
        if block_from(text, start + 4 + brace).contains(ERASED_ELEMENT) {
            names.push(name);
        }
    }
    names
}

/// Whether `struct Name` is declared in `text` with a field mentioning `AnyView<`.
/// `None` when the struct is not declared there.
fn struct_holds_any_view(text: &str, name: &str) -> Option<bool> {
    let needle = format!("struct {name}");
    for (start, _) in text.match_indices(&needle) {
        let rest = &text[start + needle.len()..];
        if rest.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let end_semi = rest.find(';').unwrap_or(rest.len());
        let open = rest.find('{').unwrap_or(rest.len());
        let body = if open < end_semi {
            block_from(rest, open)
        } else {
            &rest[..end_semi]
        };
        return Some(body.contains("AnyView<"));
    }
    None
}

/// `(view name, holds AnyView)` for every erased-element view in `files`.
fn offenders(files: &[(PathBuf, String)], crate_texts: &[String]) -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    for (path, text) in files {
        for name in erased_views(text) {
            let holds = struct_holds_any_view(text, &name).unwrap_or_else(|| {
                crate_texts
                    .iter()
                    .find_map(|t| struct_holds_any_view(t, &name))
                    .unwrap_or(false)
            });
            if holds {
                found.push((path.clone(), name));
            }
        }
    }
    found
}

/// Scan roots: the widgets crate and every plugin crate's `src`.
fn scan_roots() -> Vec<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut roots = vec![manifest.join("src")];
    if let Ok(entries) = fs::read_dir(manifest.join("../../plugins")) {
        let mut plugins: Vec<PathBuf> = entries.flatten().map(|e| e.path().join("src")).collect();
        plugins.sort();
        roots.extend(plugins.into_iter().filter(|p| p.is_dir()));
    }
    roots
}

fn load(root: &Path) -> Vec<(PathBuf, String)> {
    let mut paths = Vec::new();
    collect_rs(root, &mut paths);
    paths.sort();
    paths
        .into_iter()
        .filter_map(|p| fs::read_to_string(&p).ok().map(|s| (p, normalise(&s))))
        .collect()
}

#[test]
fn no_new_wrapper_view_holds_an_any_view_under_an_erased_element() {
    let mut bad = Vec::new();
    let mut erased_total = 0usize;
    let mut allowed_hits = Vec::new();
    for root in scan_roots() {
        let files = load(&root);
        assert!(
            !files.is_empty(),
            "no sources found under {}",
            root.display()
        );
        let crate_texts: Vec<String> = files.iter().map(|(_, t)| t.clone()).collect();
        erased_total += files
            .iter()
            .map(|(_, t)| erased_views(t).len())
            .sum::<usize>();
        for (path, name) in offenders(&files, &crate_texts) {
            if ALLOW_LIST.contains(&name.as_str()) {
                allowed_hits.push(name);
            } else {
                bad.push(format!("{} : view `{name}`", path.display()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "wrapper view(s) with `type Element = Box<dyn Widget>` holding an `AnyView<..>` field \
         hide inner type swaps from focus bookkeeping (LIMITATIONS id `{LIMITATIONS_ID}`):\n  {}",
        bad.join("\n  ")
    );
    // The scan must keep seeing the known instance, or it has gone blind.
    assert!(
        allowed_hits.iter().any(|n| n == "ReorderableListView"),
        "scan no longer finds the allow-listed ReorderableListView (scanned {erased_total} erased-element views)"
    );
}

#[test]
fn scanner_flags_the_wrapper_shape_and_ignores_others() {
    let bad = normalise(
        "pub struct Wrap<S: 'static> {\n    inner: AnyView<S>,\n}\n\
         impl<S: 'static> View<S> for Wrap<S> {\n    type Element =\n        Box<dyn Widget>;\n}\n",
    );
    // rustfmt may break the line; the normaliser joins it back.
    assert_eq!(erased_views(&bad), vec!["Wrap".to_string()]);
    assert_eq!(struct_holds_any_view(&bad, "Wrap"), Some(true));

    let ok = normalise(
        "pub struct Plain { child: Text }\n\
         impl<S: 'static> View<S> for Plain { type Element = Box<dyn Widget>; }\n\
         // impl View<S> for Doc { type Element = Box<dyn Widget>; }\n",
    );
    assert_eq!(erased_views(&ok), vec!["Plain".to_string()]);
    assert_eq!(struct_holds_any_view(&ok, "Plain"), Some(false));
}
