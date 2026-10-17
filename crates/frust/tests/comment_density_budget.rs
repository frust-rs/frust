//! Comment-density ratchet for `frust-core`: comments state contracts, not
//! narrative, and this scan keeps the cleanup from quietly regressing.
//!
//! # Scan set
//!
//! Every `.rs` file under `crates/frust-core/src` (recursive) and
//! `crates/frust-core/tests`, skipping `target/` and `workflow/`.
//!
//! # Regions
//!
//! A file splits at the first line that is exactly `#[cfg(test)]` and is
//! immediately followed by an inline module (`mod name {`, not `mod name;`): the
//! head is every line before it, the tail runs from it to the end. Otherwise —
//! an out-of-line `mod tests;` declaration included — the file is one "whole" region.
//!
//! # Rules
//!
//! - R1: comment lines (first non-blank characters `//`, so `///` and `//!`
//!   too) are at most 25.0 % of a region's lines; regions under 80 lines are
//!   exempt.
//! - R2: no run of `///`/`//` lines longer than 12; blank lines do not end a run.
//! - R3: no run of `//!` lines longer than 24; blank lines do not end a run.
//! - R4: per-file tighten-only caps in [`CAPS`] bound the head or whole region
//!   (measured ratio plus 3.0 points). Like R1 it applies to regions of 80+ lines.
//!
//! The awk sweep that produced the measurements counts the same way; the
//! fixtures below encode its semantics.

use std::fs;
use std::path::{Path, PathBuf};

const MAX_RATIO: f32 = 25.0;
const MIN_REGION_LINES: usize = 80;
const MAX_DOC_RUN: usize = 12;
const MAX_HEADER_RUN: usize = 24;

/// Per-file ceilings on the head (or whole) region's comment percentage, each
/// the measured value on the post-cleanup tree plus 3.0 points, capped at 25.0.
/// Lower an entry when you trim a file; raising one needs a
/// `docs/CODE_STANDARDS.md` change. Files absent from the table get 25.0.
const CAPS: &[(&str, f32)] = &[
    ("crates/frust-core/src/anim.rs", 25.0), // measured 22.5
    ("crates/frust-core/src/app/config.rs", 25.0), // measured 22.5
    ("crates/frust-core/src/app/dispatch.rs", 25.0), // measured 23.2
    ("crates/frust-core/src/app/layout.rs", 21.1), // measured 18.1
    ("crates/frust-core/src/app/metrics.rs", 25.0), // measured 25.0
    ("crates/frust-core/src/app/mod.rs", 25.0), // measured 24.8
    ("crates/frust-core/src/app/paint.rs", 25.0), // measured 24.7
    ("crates/frust-core/src/app/rebuild.rs", 25.0), // measured 24.6
    ("crates/frust-core/src/app/semantics.rs", 25.0), // measured 23.5
    ("crates/frust-core/src/app/session.rs", 25.0), // measured 24.5
    ("crates/frust-core/src/app/tests/clipboard.rs", 15.7), // measured 12.7
    ("crates/frust-core/src/app/tests/contacts.rs", 9.6), // measured 6.6
    ("crates/frust-core/src/app/tests/cursor.rs", 14.3), // measured 11.3
    ("crates/frust-core/src/app/tests/file_drop.rs", 9.1), // measured 6.1
    ("crates/frust-core/src/app/tests/focus_ime.rs", 15.4), // measured 12.4
    ("crates/frust-core/src/app/tests/hover.rs", 18.1), // measured 15.1
    ("crates/frust-core/src/app/tests/lifecycle.rs", 11.7), // measured 8.7
    ("crates/frust-core/src/app/tests/mod.rs", 16.8), // measured 13.8
    ("crates/frust-core/src/app/tests/overlay.rs", 14.3), // measured 11.3
    ("crates/frust-core/src/app/tests/shortcuts.rs", 6.5), // measured 3.5
    ("crates/frust-core/src/app/tests/platform_views.rs", 14.6), // measured 11.6
    ("crates/frust-core/src/app/tests/scale.rs", 13.9), // measured 10.9
    ("crates/frust-core/src/app/tests/selection_toolbar.rs", 21.4), // measured 18.4
    ("crates/frust-core/src/app/tests/semantics.rs", 12.0), // measured 9.0
    ("crates/frust-core/src/component.rs", 25.0), // measured 23.7
    ("crates/frust-core/src/event/contact.rs", 23.8), // measured 20.8
    ("crates/frust-core/src/event/ctx.rs", 25.0), // measured 24.0
    ("crates/frust-core/src/event/flags.rs", 25.0), // measured 24.4
    ("crates/frust-core/src/event/ime.rs", 25.0), // measured 24.2
    ("crates/frust-core/src/event/input.rs", 25.0), // measured 24.5
    ("crates/frust-core/src/event/key.rs", 24.2), // measured 21.2
    ("crates/frust-core/src/event/mod.rs", 25.0), // measured 38.6 over 57 lines (size-exempt)
    ("crates/frust-core/src/event/pass.rs", 25.0), // measured 22.3
    ("crates/frust-core/src/event/pointer.rs", 25.0), // measured 24.1
    ("crates/frust-core/src/event/tests.rs", 10.7), // measured 7.7
    ("crates/frust-core/src/hotpatch.rs", 24.6), // measured 21.6
    ("crates/frust-core/src/input.rs", 22.8), // measured 19.8
    ("crates/frust-core/src/insets.rs", 25.0), // measured 23.0
    ("crates/frust-core/src/layout.rs", 19.0), // measured 16.0
    ("crates/frust-core/src/lib.rs", 20.9),  // measured 17.9
    ("crates/frust-core/src/overlay.rs", 25.0), // measured 24.6
    ("crates/frust-core/src/shortcuts.rs", 19.0), // measured 16.0
    ("crates/frust-core/src/retained.rs", 25.0), // measured 23.4
    ("crates/frust-core/src/selection_toolbar.rs", 25.0), // measured 23.7
    ("crates/frust-core/src/semantics.rs", 24.4), // measured 21.4
    ("crates/frust-core/src/sliver.rs", 25.0), // measured 22.5
    ("crates/frust-core/src/tree.rs", 25.0), // measured 22.9
    ("crates/frust-core/src/view.rs", 25.0), // measured 24.8
    ("crates/frust-core/src/widget.rs", 23.6), // measured 20.6
    ("crates/frust-core/tests/layout_audit.rs", 10.9), // measured 7.9
    ("crates/frust-core/tests/layout_skip.rs", 17.2), // measured 14.2
    ("crates/frust-core/tests/paint_dirty.rs", 14.4), // measured 11.4
    ("crates/frust-core/tests/retained_paint.rs", 8.1), // measured 5.1
    ("crates/frust-core/tests/sliver_seam.rs", 12.6), // measured 9.6
];

fn cap_for(path: &str) -> f32 {
    CAPS.iter()
        .find(|(p, _)| *p == path)
        .map_or(MAX_RATIO, |(_, cap)| *cap)
}

struct Region<'a> {
    name: &'static str,
    lines: Vec<&'a str>,
}

/// `mod name;` only declares a module; any other `mod name …` line opens it inline.
fn is_inline_mod(line: &str) -> bool {
    line.starts_with("mod ") && !line.trim_end().ends_with(';')
}

fn regions(src: &str) -> Vec<Region<'_>> {
    let lines: Vec<&str> = src.lines().collect();
    let split = (0..lines.len().saturating_sub(1))
        .find(|&i| lines[i] == "#[cfg(test)]" && is_inline_mod(lines[i + 1]));
    match split {
        Some(i) => vec![
            Region {
                name: "head",
                lines: lines[..i].to_vec(),
            },
            Region {
                name: "tail",
                lines: lines[i..].to_vec(),
            },
        ],
        None => vec![Region {
            name: "whole",
            lines,
        }],
    }
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

fn is_header(line: &str) -> bool {
    line.trim_start().starts_with("//!")
}

fn ratio(lines: &[&str]) -> f32 {
    if lines.is_empty() {
        return 0.0;
    }
    100.0 * lines.iter().filter(|l| is_comment(l)).count() as f32 / lines.len() as f32
}

/// Longest `(doc/plain run, header run)`; a run survives blank lines and ends
/// at any code line or at a line of the other comment class.
fn max_runs(lines: &[&str]) -> (usize, usize) {
    let (mut block, mut head, mut max_block, mut max_head) = (0, 0, 0, 0);
    for line in lines {
        if is_header(line) {
            head += 1;
            block = 0;
            max_head = max_head.max(head);
        } else if is_comment(line) {
            block += 1;
            head = 0;
            max_block = max_block.max(block);
        } else if !line.trim().is_empty() {
            block = 0;
            head = 0;
        }
    }
    (max_block, max_head)
}

/// Violations as `path region rule measured limit` lines.
fn check(path: &str, src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for region in regions(src) {
        let n = region.lines.len();
        let name = region.name;
        let r = ratio(&region.lines);
        if n >= MIN_REGION_LINES {
            if r > MAX_RATIO {
                out.push(format!("{path} {name} R1 {r:.1} {MAX_RATIO:.1}"));
            }
            let cap = cap_for(path);
            if name != "tail" && r > cap {
                out.push(format!("{path} {name} R4 {r:.1} {cap:.1}"));
            }
        }
        let (block, head) = max_runs(&region.lines);
        if block > MAX_DOC_RUN {
            out.push(format!("{path} {name} R2 {block} {MAX_DOC_RUN}"));
        }
        if head > MAX_HEADER_RUN {
            out.push(format!("{path} {name} R3 {head} {MAX_HEADER_RUN}"));
        }
    }
    out
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/frust has a grandparent (the workspace root)")
        .to_path_buf()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("dir entry under {}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            let skip = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == "target" || n == "workflow");
            if !skip {
                collect(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn frust_core_comment_density_within_budget() {
    let root = workspace_root();
    let mut files = Vec::new();
    for sub in ["crates/frust-core/src", "crates/frust-core/tests"] {
        let dir = root.join(sub);
        assert!(
            dir.is_dir(),
            "expected scan root {} to exist",
            dir.display()
        );
        collect(&dir, &mut files);
    }
    files.sort();
    assert!(!files.is_empty(), "no frust-core sources found");

    let mut violations = Vec::new();
    for file in &files {
        let rel = file
            .strip_prefix(&root)
            .expect("scanned file is under the workspace root")
            .to_string_lossy()
            .replace('\\', "/");
        let src =
            fs::read_to_string(file).unwrap_or_else(|e| panic!("reading {}: {e}", file.display()));
        violations.extend(check(&rel, &src));
    }
    assert!(
        violations.is_empty(),
        "comment-density budget violated (path region rule measured limit):\n{}\n\n\
         See docs/CODE_STANDARDS.md § Comment Conventions: comments state contracts, \
         not narrative. Trim the offending comments rather than raising a limit. \
         For R4, lower the CAPS entry when you trim a file; raising one needs a \
         docs/CODE_STANDARDS.md change.",
        violations.join("\n")
    );
}

#[test]
fn every_scanned_file_has_a_caps_entry() {
    let root = workspace_root();
    let mut files = Vec::new();
    for sub in ["crates/frust-core/src", "crates/frust-core/tests"] {
        collect(&root.join(sub), &mut files);
    }
    for file in files {
        let rel = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        assert!(
            CAPS.iter().any(|(p, _)| *p == rel),
            "{rel} has no CAPS entry"
        );
    }
    for (p, cap) in CAPS {
        assert!(root.join(p).is_file(), "CAPS lists missing file {p}");
        assert!(*cap <= MAX_RATIO, "{p} cap above {MAX_RATIO}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(line: &str, n: usize) -> String {
        vec![line; n].join("\n")
    }

    #[test]
    fn splits_at_cfg_test_mod() {
        let r = regions("fn a() {}\n#[cfg(test)]\nmod tests {}\n");
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].name, r[0].lines.len()), ("head", 1));
        assert_eq!((r[1].name, r[1].lines.len()), ("tail", 2));
    }

    #[test]
    fn out_of_line_mod_declaration_is_not_a_split() {
        let r = regions("#[cfg(test)]\nmod tests;\nfn a() {}\n");
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].name, r[0].lines.len()), ("whole", 3));
    }

    #[test]
    fn no_split_is_whole() {
        let r = regions("fn a() {}\nfn b() {}\n");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "whole");
    }

    #[test]
    fn cfg_test_not_followed_by_mod_is_not_a_split() {
        let r = regions("#[cfg(test)]\nfn helper() {}\n    #[cfg(test)]\n    mod m {}\n");
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn r1_exempt_under_80_lines_but_enforced_at_80() {
        let small = rep("// c", 79);
        assert!(!check("x.rs", &small).iter().any(|v| v.contains(" R1 ")));
        let big = format!("{}\n{}", rep("// c", 40), rep("fn f() {}", 40));
        assert!(check("x.rs", &big).iter().any(|v| v.contains(" R1 ")));
    }

    #[test]
    fn r2_run_continues_across_blank_and_doc_to_plain() {
        let src = format!("{}\n\n{}\nfn f() {{}}", rep("/// a", 6), rep("// b", 7));
        assert!(check("x.rs", &src).iter().any(|v| v.contains(" R2 13 12")));
        let ok = format!("{}\n\n{}\nfn f() {{}}", rep("/// a", 6), rep("// b", 6));
        assert!(check("x.rs", &ok).is_empty());
    }

    #[test]
    fn r2_run_ends_at_code() {
        let src = format!("{}\nfn f() {{}}\n{}", rep("/// a", 12), rep("/// b", 12));
        assert!(check("x.rs", &src).is_empty());
    }

    #[test]
    fn r3_header_run_limit() {
        let ok = format!("{}\nfn f() {{}}", rep("//! h", 24));
        assert!(check("x.rs", &ok).is_empty());
        let bad = format!("{}\n\n{}", rep("//! h", 20), rep("//! h", 5));
        assert!(check("x.rs", &bad).iter().any(|v| v.contains(" R3 25 24")));
    }

    #[test]
    fn header_and_doc_classes_do_not_combine() {
        let src = format!("{}\n{}\nfn f() {{}}", rep("//! h", 20), rep("/// d", 12));
        assert!(check("x.rs", &src).is_empty());
        let src = format!("{}\n{}\nfn f() {{}}", rep("/// d", 12), rep("//! h", 20));
        assert!(check("x.rs", &src).is_empty());
    }

    #[test]
    fn passing_fixture() {
        let src = "//! Header.\n\n/// Doc.\nfn f() {}\n";
        assert!(check("x.rs", src).is_empty());
    }

    #[test]
    fn caps_lookup_defaults_to_ceiling() {
        assert_eq!(cap_for("crates/frust-core/src/not_listed.rs"), MAX_RATIO);
        assert!(cap_for(CAPS[0].0) <= MAX_RATIO);
    }

    #[test]
    fn r4_cap_applies_to_head_only() {
        let body = format!("{}\n{}", rep("// c", 10), rep("fn f() {}", 70));
        let (path, cap) = CAPS
            .iter()
            .find(|(_, c)| *c < MAX_RATIO)
            .expect("a tight cap");
        let tight = format!(
            "{}\n{}\n#[cfg(test)]\nmod t {{}}\n",
            rep("// c", *cap as usize + 5),
            rep("fn f() {}", 100 - *cap as usize)
        );
        assert!(check(path, &tight).iter().any(|v| v.contains(" R4 ")));
        assert!(check("x.rs", &body).is_empty());
    }
}
