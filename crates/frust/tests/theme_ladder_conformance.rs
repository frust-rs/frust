//! Source-scan conformance test for the three shells' default-theme
//! precedence ladder (`docs/ARCHITECTURE.md`'s Theme delivery): app override >
//! design-system-seeded default > the shell's built-in `Theme::neutral()`
//! fallback.
//!
//! # Why a source scan
//!
//! The ladder's arms are per-shell private functions, and only the desktop
//! shell's unit tests run in a host `cargo test --workspace`: both mobile
//! shells' `app` modules are `#[cfg(target_os = "android"/"ios")]`, so their
//! `#[cfg(test)]` twins are neither compiled by the documented compile gates
//! (`cargo check --target ... -p frust-ui`, which never builds test cfg) nor
//! runnable without a device. Left at that, a regression in either mobile arm
//! would ship through a fully green gate — the exact hole this file closes.
//!
//! Precedent: `crates/frust/tests/surface_mode_conformance.rs` and
//! `frust-drive/tests/print_free_cores.rs` (plain `std::fs` source scans run as
//! ordinary `cargo test`s — this repo has no lint-plugin tooling, see
//! `docs/CODE_STANDARDS.md`).
//!
//! # What this checks
//!
//! 1. **No shell's production code calls `Theme::neutral()`.** The
//!    built-in fallback is reachable only through `base_theme`'s
//!    `unwrap_or_else(Theme::neutral)`, so a seed or reset site that
//!    reverts to an unconditional `Theme::neutral()` — silently
//!    discarding a design system's seeded default — fails here.
//! 2. **Each shell's seed site still reads the default slot**: exactly one
//!    `base_theme(default_theme())` per shell. This is the hole the shells'
//!    own unit tests structurally cannot see — they call `base_theme(None)`
//!    and `base_theme(Some(..))` directly, so a *production* seed rewritten to
//!    `base_theme(None)` (disabling `set_default_theme` outright, the whole
//!    point of the `theme_default` seam) keeps every one of them green. Only
//!    the desktop shell even has a seed-site test, and it re-types the
//!    composition rather than sharing it with the arm.
//! 3. **Each shell's override-poll and appearance arms still route through the
//!    extracted helpers**, exactly once each: deleting the call and inlining
//!    the ladder back into the arm fails here even if the helper itself is
//!    left behind, unused.
//! 4. **No shell names the Glyph design system.** Glyph ships as its own
//!    plugin crate no shell depends on, so neither `Theme::glyph_baseline()`
//!    (a constructor that no longer exists anywhere) nor `frust_theme::glyph::*`
//!    may appear in shell *code* — the Glyph tokens and their bundled fonts
//!    arrive through the same `set_default_theme` + `register_app_fonts` seams
//!    as any other design system's. The needles are deliberately kept as a
//!    ban on the *spellings*, not just on what currently compiles: a
//!    re-introduction attempt fails here with a reason rather than as a bare
//!    unresolved-path error. Prose mentions are fine (the scan strips
//!    comments).
//! 5. **The three shells' ladder helpers are byte-identical** (modulo comments
//!    and whitespace). The desktop copy is the one covered by real, host-run
//!    unit tests (`frust-shell-desktop`'s `app_handler::tests`); this pins the
//!    two mobile copies to it, so a divergence in a body no host test can
//!    reach is a failure here rather than a device-only surprise.
//!
//! # What this is NOT
//!
//! A substring/line scan, not a parser — comment-only lines and trailing `//`
//! comments are stripped, which is correct for this codebase because every
//! call in scope sits on its own statement line and no helper body contains a
//! string literal. Check 4 widens the same scan to a whole shell file
//! (production plus test module), so a *string literal* there that happened to
//! spell a banned name would count as code — a false positive with an obvious
//! message, preferred over teaching this scan to lex Rust.

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

/// The three shell sources that own a copy of the ladder, workspace-relative.
const SHELL_SOURCES: &[&str] = &[
    "crates/frust-shell-desktop/src/app_handler.rs",
    "crates/frust-shell-android/src/app.rs",
    "crates/frust-shell-ios/src/app/theme.rs",
];

/// The ladder's helpers, in the order a reader meets them. Each must exist,
/// exactly once, in all three shells with an identical body.
const LADDER_HELPERS: &[&str] = &[
    "base_theme",
    "reverted_theme",
    "theme_after_override_poll",
    "follow_platform_brightness",
];

fn read_shell(rel: &str) -> String {
    let path = workspace_root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// Everything before the file's `#[cfg(test)]` line — the code that actually
/// ships. Each shell source has exactly one such module, at the end; that is
/// asserted here rather than assumed, since a second one appearing earlier
/// would silently shrink every scan below.
fn production_region(rel: &str, contents: &str) -> String {
    let markers: Vec<usize> = contents
        .lines()
        .enumerate()
        .filter(|(_, line)| line.trim() == "#[cfg(test)]")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        markers.len(),
        1,
        "{rel} must have exactly one `#[cfg(test)]` module for this scan to split \
         production from test code; found {} — teach `production_region` about the \
         new shape before relying on the assertions below",
        markers.len()
    );
    contents
        .lines()
        .take(markers[0])
        .collect::<Vec<_>>()
        .join("\n")
}

/// `line` with any trailing `//` comment removed, then trimmed. Correct here
/// because no line in scope embeds `//` inside a string literal.
fn strip_comment(line: &str) -> &str {
    match line.find("//") {
        Some(at) => line[..at].trim(),
        None => line.trim(),
    }
}

/// Real (non-comment) occurrences of `needle` in `src`, as `line-number: text`
/// entries for a failure message.
fn hits(src: &str, needle: &str) -> Vec<String> {
    src.lines()
        .enumerate()
        .filter(|(_, line)| strip_comment(line).contains(needle))
        .map(|(i, line)| format!("  line {}: {}", i + 1, line.trim()))
        .collect()
}

/// The source lines of `fn <name>(` through its closing brace, comment- and
/// whitespace-normalized into one string for cross-shell comparison. The
/// leading doc comment is deliberately excluded: the three shells document the
/// same logic against different platform callbacks.
fn helper_body(rel: &str, src: &str, name: &str) -> String {
    let signature = format!("fn {name}(");
    let start = src
        .lines()
        .position(|line| line.trim_start().starts_with(&signature))
        .unwrap_or_else(|| panic!("{rel} must define `{signature}...)` — the ladder helper"));

    let mut depth: i32 = 0;
    let mut opened = false;
    let mut normalized: Vec<String> = Vec::new();
    for line in src.lines().skip(start) {
        let code = strip_comment(line);
        if !code.is_empty() {
            normalized.push(code.split_whitespace().collect::<Vec<_>>().join(" "));
        }
        depth += code.matches('{').count() as i32 - code.matches('}').count() as i32;
        if depth > 0 {
            opened = true;
        }
        if opened && depth == 0 {
            return normalized.join(" ");
        }
    }
    panic!("{rel}'s `{signature}...)` has no closing brace — unbalanced source?");
}

/// Check 1: the built-in fallback is reachable only through `base_theme`.
///
/// `base_theme`'s own `seeded.unwrap_or_else(Theme::neutral)` passes the
/// constructor as a *value* (no parentheses), so this ban on the *call* form
/// leaves the one sanctioned use alone while catching every seed/reset site
/// that hardcodes the fallback and throws a design system's seeded default
/// away.
#[test]
fn no_shell_hardcodes_the_builtin_fallback_outside_base_theme() {
    for rel in SHELL_SOURCES {
        let contents = read_shell(rel);
        let production = production_region(rel, &contents);
        let found = hits(&production, "Theme::neutral()");
        assert!(
            found.is_empty(),
            "{rel} calls `Theme::neutral()` in production code ({} site(s)). The \
             built-in fallback belongs behind `base_theme`'s `unwrap_or_else` alone — a \
             seed or reset site that constructs it directly ignores a design system's \
             `set_default_theme` (docs/ARCHITECTURE.md's Theme delivery):\n{}",
            found.len(),
            found.join("\n"),
        );
    }
}

/// Check 2: the seed site really reads the design-system default slot.
///
/// The gap this closes: every ladder unit test calls `base_theme` with a value
/// it supplies itself, so none of them can observe what the *production* seed
/// passes in. A seed rewritten to `base_theme(None)` still compiles, still
/// drives the same helper, and still leaves the whole ladder suite green — while
/// silently making `set_default_theme` a no-op at the one site it exists for.
/// Exactly one occurrence is expected per shell: its single seed site
/// (`run_desktop`'s `theme:` field, `AndroidAppHandle::new`,
/// `IosAppHandle::new`).
///
/// The `clear_app_theme` reset arm is pinned separately — it passes
/// `default_theme` as a *supplier* into `theme_after_override_poll` (check 3),
/// deliberately lazily, so it does not spell this call form.
#[test]
fn every_shell_seeds_base_theme_from_the_default_slot() {
    for rel in SHELL_SOURCES {
        let contents = read_shell(rel);
        let production = production_region(rel, &contents);
        let found = hits(&production, "base_theme(default_theme())");
        assert_eq!(
            found.len(),
            1,
            "{rel} must contain exactly one `base_theme(default_theme())` occurrence in \
             production code — the shell's single seed site. A seed that stops passing \
             `default_theme()` (say `base_theme(None)`) disables `set_default_theme` \
             entirely while every ladder unit test stays green, because those tests supply \
             `base_theme`'s argument themselves (docs/ARCHITECTURE.md's Theme delivery); \
             found {}:\n{}",
            found.len(),
            found.join("\n"),
        );
    }
}

/// Check 3: the per-frame override poll really goes through the extracted
/// decision, in every shell.
///
/// Two occurrences are expected per file — the `fn` definition and the single
/// call from the shell's frame/redraw arm — so an arm that inlines the ladder
/// again (leaving the helper defined but uncalled) drops to one and fails.
#[test]
fn every_shell_routes_its_override_poll_through_the_shared_decision() {
    for rel in SHELL_SOURCES {
        let contents = read_shell(rel);
        let production = production_region(rel, &contents);
        let found = hits(&production, "theme_after_override_poll(");
        assert_eq!(
            found.len(),
            2,
            "{rel} must contain exactly two `theme_after_override_poll(` occurrences in \
             production code — its definition and the one per-frame call site — so the \
             `clear_app_theme` arm and the unit tests share an implementation; found \
             {}:\n{}",
            found.len(),
            found.join("\n"),
        );
    }
}

/// Check 3b: the platform-appearance arm likewise.
#[test]
fn every_shell_routes_its_appearance_change_through_the_shared_decision() {
    for rel in SHELL_SOURCES {
        let contents = read_shell(rel);
        let production = production_region(rel, &contents);
        let found = hits(&production, "follow_platform_brightness(");
        assert_eq!(
            found.len(),
            2,
            "{rel} must contain exactly two `follow_platform_brightness(` occurrences in \
             production code — its definition and the one appearance-change call site; \
             found {}:\n{}",
            found.len(),
            found.join("\n"),
        );
    }
}

/// Check 4: no shell names the Glyph design system, in production code OR in
/// its own test module.
///
/// The Glyph design system lives in its own plugin crate (`frust-glyph`), which
/// no shell depends on; the framework constructs no Glyph tokens at all. A
/// shell that spells `Theme::glyph_baseline()` or reaches into
/// `frust_theme::glyph::*` therefore names nothing that exists — this check
/// bans the spellings so the failure names the rule instead of surfacing as an
/// unresolved path. The mobile
/// shells' test modules matter as much as their production code here: they are
/// only type-checked by an explicit `--all-targets` cross-check
/// (`docs/DEVELOPMENT.md`), so a stale reference there would otherwise sit
/// undetected. The whole file is scanned for that reason, not just the
/// production region.
///
/// Comment-stripped, so prose may still discuss Glyph (this repo's shells carry
/// incidental "glyph outlines"/"sharp glyphs" text-rendering comments that have
/// nothing to do with the design system).
#[test]
fn no_shell_names_the_glyph_design_system() {
    const GLYPH_NEEDLES: &[&str] = &["glyph_baseline", "frust_theme::glyph", "theme::glyph::"];
    for rel in SHELL_SOURCES {
        let contents = read_shell(rel);
        for needle in GLYPH_NEEDLES {
            let found = hits(&contents, needle);
            assert!(
                found.is_empty(),
                "{rel} names `{needle}` in code ({} site(s)). No shell may depend on the \
                 Glyph design system: it lives in its own plugin crate (`frust-glyph`), \
                 and a shell's built-in fallback is `Theme::neutral()`. A design system \
                 reaches a shell through `set_default_theme` + `register_app_fonts`, \
                 never the other way round:\n{}",
                found.len(),
                found.join("\n"),
            );
        }
    }
}

/// Check 5: the three copies stay in lockstep, so the desktop shell's host-run
/// unit tests transitively cover the mobile arms.
#[test]
fn the_three_shells_ladder_helpers_are_identical() {
    let sources: Vec<(&str, String)> = SHELL_SOURCES
        .iter()
        .map(|rel| (*rel, read_shell(rel)))
        .collect();
    let (reference_rel, reference_src) = &sources[0];

    for helper in LADDER_HELPERS {
        let expected = helper_body(reference_rel, reference_src, helper);
        for (rel, src) in sources.iter().skip(1) {
            let actual = helper_body(rel, src, helper);
            assert_eq!(
                actual, expected,
                "`fn {helper}` has drifted between {reference_rel} and {rel}. The three \
                 shells deliberately keep their own copy (see the task note against \
                 unifying them), but only the desktop copy is covered by host-run unit \
                 tests — a mobile body that differs is a behavior no `cargo test \
                 --workspace` can reach.\n  {reference_rel}: {expected}\n  {rel}: {actual}"
            );
        }
    }
}

/// The scan's own blind-spot check: `strip_comment` must not let a commented-out
/// or prose mention of a banned call count as a hit, and must still see real
/// code that happens to carry a trailing comment.
#[test]
fn the_comment_stripper_sees_code_and_ignores_prose() {
    let planted = "\
// prose about Theme::neutral() in a doc comment
let a = Theme::neutral(); // a real call with a trailing comment
    // an indented comment-only line mentioning Theme::neutral()
";
    let found = hits(planted, "Theme::neutral()");
    assert_eq!(
        found.len(),
        1,
        "expected exactly the one real call to be reported, got:\n{}",
        found.join("\n")
    );
    assert!(found[0].contains("let a ="));
}
