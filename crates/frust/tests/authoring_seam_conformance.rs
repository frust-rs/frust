//! Source-scan conformance test locking the `frust::authoring` escape hatch
//! shut for every migrated production consumer: no `benchmarks/frust_bench`,
//! `examples/huddle`, `examples/shadertoy`, `examples/glyph-catalog`, or
//! `examples/layer-bench` production source file may name `frust_core`,
//! `frust_scene`, `frust_text`, `kurbo`, or `peniko` as a crate path. Those
//! five app-tier consumers were migrated onto `frust::authoring` (this
//! feature's own `PLAN.md`); this test is what makes reopening that escape
//! hatch a build failure instead of a silent regression the next PR review
//! has to catch by eye.
//!
//! Precedent (match structure/error-message style/doc-comment depth):
//! - `crates/frust-widgets/tests/authoring_only_conformance.rs` — the
//!   analogous "only reach container plumbing through the public authoring
//!   path" scan for the three built-in widget catalogs. This test's directory
//!   walk, `workspace_root()`/`rel()` helpers, and loud-failure-on-missing-
//!   directory posture are lifted directly from it.
//! - `crates/frust/tests/surface_mode_conformance.rs` — the same pattern one
//!   layer up (facade-level), including its multi-line-brace-group tracking
//!   for a `pub use` statement; this test reuses that exact technique for a
//!   `use frust::{ ... }` import group (see "The `frust::`-prefixed valve"
//!   below).
//!
//! # What this checks
//!
//! Every `.rs` file under each of the five consumers' `src/` trees (see
//! [`CONSUMER_SRC_DIRS`]) is scanned for:
//!
//! 1. Any bare reference to `frust_core::`, `frust_scene::`, or
//!    `frust_text::` — these three crates are never legitimately named by an
//!    app-tier consumer; every path through them is fully covered by
//!    `frust::authoring`/`frust::authoring::text`/`frust::authoring::scene`.
//! 2. Any bare reference to `kurbo::` or `peniko::` that is NOT itself part
//!    of the `frust::kurbo`/`frust::peniko` long-tail valve (`crates/frust/
//!    src/lib.rs`'s whole-crate re-exports, for the handful of types
//!    `authoring` does not lift by name) — see "The `frust::`-prefixed valve"
//!    below for how that distinction is made.
//!
//! # The `frust::`-prefixed valve
//!
//! `kurbo`/`peniko`, unlike the other three crates, have a SANCTIONED bare
//! spelling: `frust::kurbo::X`/`frust::peniko::X` (this crate's own
//! `pub use kurbo;`/`pub use peniko;`), used for the long tail of types
//! `frust::authoring` does not lift by name (see this feature's `PLAN.md`
//! §Conductor Amendments for the exact by-name list: `Affine, BezPath, Line,
//! Point, Rect, RoundedRect, Shape, Size, Stroke, Vec2` from kurbo; `Brush,
//! Color, Fill, ImageData` from peniko — everything else goes through the
//! valve). A scan for a bare `kurbo::`/`peniko::` substring would false-flag
//! every one of these legitimate uses, so a reference only counts as a
//! violation when it is NEITHER:
//!
//! - immediately prefixed by `frust::` on the same line (`frust::kurbo::
//!   Circle`, `frust::peniko::color::DynamicColor`), NOR
//! - a sub-item inside an active multi-line `use frust::{ ... };` brace
//!   group (e.g. `examples/huddle/.../thread.rs`'s `use frust::{ ..., keyed,
//!   kurbo::Size, scroll_view, ... };` — the literal text on `kurbo::Size`'s
//!   own line has no `frust::` immediately before it, but the enclosing
//!   group's root IS `frust::`, so this is the same valve reference split
//!   across lines). This scan tracks that group exactly like
//!   `surface_mode_conformance.rs` tracks a multi-line `pub use` group:
//!   brace-depth from the opening `use frust::` line to its terminating `;`.
//!
//! `frust_core`/`frust_scene`/`frust_text` get no such valve — there is no
//! `frust::frust_core` re-export, so any bare occurrence of these three is a
//! violation regardless of surrounding context.
//!
//! # What is excluded, and why (each a correctness requirement)
//!
//! - **`plugins/**`** — not scanned at all (outside [`CONSUMER_SRC_DIRS`]).
//!   `plugins/native-widgets` imports `frust_core`/`kurbo` in production and
//!   MUST keep doing so: `docs/ARCHITECTURE.md`'s facade/plugin boundary
//!   places plugins beside the facade, never inside it, so the migration this
//!   test locks in was never meant to reach plugins.
//! - **`crates/**`** — not scanned at all. The framework's own crates
//!   legitimately depend on each other (`frust-core` on `frust-scene`, etc.);
//!   the seam this test locks is app-tier-consumer-facing only. This also
//!   means this test's OWN file — full of the forbidden strings, in its
//!   error messages, doc comments, and unit-test fixtures below — can never
//!   self-match: `crates/frust/tests/` is under `crates/**`.
//! - **`**/tests/**`, `**/benches/**`** — never walked into (checked on every
//!   directory-name component, not just the scan root, in case a future
//!   consumer nests one under `src/`). Migrations were scoped to production
//!   deps only; several consumers keep legitimate `[dev-dependencies]` on
//!   these crates for a headless `RenderRoot`-driving test harness (e.g.
//!   `examples/glyph-catalog/Cargo.toml`'s own comment on its `tests/
//!   smoke.rs` dev-deps: "`RenderRoot` is deliberately not in
//!   `frust::authoring`... production code names none of them").
//! - **`#[cfg(test)]` modules** — every real line inside one is skipped by
//!   brace-depth tracking from the `#[cfg(test)]` attribute's very next line
//!   (a `mod ... {`) to its matching closing brace. Same dev-only reasoning
//!   as the previous bullet, just inlined instead of a separate `tests/`
//!   file — confirmed present and skipped in this migrated tree at
//!   `examples/huddle/src/ui/sheet.rs` (~line 973: `frust_core::{BuildCtx,
//!   LayoutCtx, PaintCtx, RenderRoot, WindowEdgeInsets, WindowInsets}`,
//!   `kurbo::Size`), `examples/huddle/src/ui/toast.rs` (~line 372:
//!   `frust_core::{FrameTime, RenderRoot}`, `peniko::Color`), and
//!   `examples/glyph-catalog/src/pages/interactions.rs` (~line 1455:
//!   `frust_scene::GlyphRun`, `frust_text::TextContext`, `kurbo::{BezPath,
//!   Point, Rect, Size}`, `peniko::{Brush, Color}`).
//!
//! # What this is NOT
//!
//! A substring/line scan, not a parser — matching `authoring_only_
//! conformance.rs`/`surface_mode_conformance.rs`. Comment-only lines (`//`/
//! `//!`/`///`, trimmed) are stripped BEFORE any check runs, deliberately:
//! several migrated files legitimately *mention* these crate names in prose
//! (`examples/glyph-catalog/src/pages/navigation.rs`'s module doc explains,
//! in comment text, why huddle's icon path carries a one-off `frust-core`/
//! `kurbo`/`peniko` escape-hatch dependency that this catalog does NOT
//! repeat) — flagging that prose would be a false positive annoying every
//! future contributor. String literals are not stripped separately: checked
//! against the actual tree (see this module's own development — no
//! consumer's production `src/` currently spells one of these five crate
//! names inside a string literal), so a dedicated string-literal exclusion
//! would add complexity with nothing in this tree for it to protect against.
//! Correct for this codebase because, as with the two precedents, every real
//! reference in scope sits on its own statement line.

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

/// The five migrated production consumers this scan locks the authoring seam
/// against, each a `src/` directory relative to the workspace root.
/// `benchmarks/frust_bench` sits inside the root workspace graph;
/// the four `examples/*` entries are standalone workspaces (their own
/// `[workspace]`, own `Cargo.lock` — root `Cargo.toml`'s `exclude` list)
/// reached here purely by relative path from `CARGO_MANIFEST_DIR`, the same
/// way any other source-scan conformance test in this repo reaches outside
/// its own crate.
const CONSUMER_SRC_DIRS: &[&str] = &[
    "benchmarks/frust_bench/src",
    "examples/huddle/src",
    "examples/shadertoy/src",
    "examples/glyph-catalog/src",
    "examples/layer-bench/src",
];

/// True if `name` (a single path component) is a directory this scan must
/// never walk into: dev-only test/bench sources kept out of the migration's
/// scope (see this module's doc comment).
fn is_excluded_dir_component(name: &str) -> bool {
    name == "tests" || name == "benches"
}

/// Recursively collects every `.rs` file under `dir` into `out`, skipping any
/// subdirectory named `tests`/`benches` at any depth.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|e| panic!("dir entry under {}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            let is_excluded = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_excluded_dir_component);
            if is_excluded {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Every `.rs` file across the five consumers' `src/` trees, sorted for a
/// stable failure order. Panics loudly — rather than silently scanning zero
/// files — if a scanned directory is missing (moved/renamed/not-yet-migrated
/// consumer removed from the list) or if the total looks implausibly small
/// for five real app-tier consumers; this is the failure mode the task
/// singles out as the one that matters most (a green test proving nothing).
fn consumer_source_files() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut out = Vec::new();
    for consumer in CONSUMER_SRC_DIRS {
        let dir = root.join(consumer);
        assert!(
            dir.is_dir(),
            "expected consumer source directory {} to exist — has it moved, been renamed, or was \
             a consumer removed from CONSUMER_SRC_DIRS without updating this test? This scan must \
             fail loudly here, not silently pass over zero files.",
            dir.display()
        );
        let before = out.len();
        rust_files(&dir, &mut out);
        assert!(
            out.len() > before,
            "expected at least one `.rs` file under {} — found none. A silently-zero scan that \
             passes because it found no files is exactly the false-confidence failure mode this \
             test exists to avoid.",
            dir.display()
        );
    }
    out.sort();
    assert!(
        out.len() > 100,
        "expected well over 100 production source files across the five migrated consumers, \
         found {} — the scan is probably looking in the wrong place",
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

/// True if `line`, trimmed, is a comment-only line (mirrors
/// `authoring_only_conformance.rs`/`surface_mode_conformance.rs`'s comment
/// stripping — see this module's own "What this is NOT" doc for why prose
/// mentions of these crate names must not count as violations).
fn is_comment_only(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// The crates with NO sanctioned bare spelling for an app-tier consumer:
/// every path through them is fully covered by `frust::authoring`/
/// `frust::authoring::text`/`frust::authoring::scene`, so a bare reference is
/// always a violation, unlike `kurbo`/`peniko` below.
///
/// `accesskit` earns its place here for a subtler reason than the other three.
/// It is the one crate a widget could otherwise be *forced* to name directly:
/// `SemanticsCtx::push_node`/`push_container` take `accesskit::Role` and
/// `&mut accesskit::Node`, so any widget contributing (not merely forwarding)
/// an accessibility node must spell those types. `frust::authoring` lifts
/// `Node`/`NodeId`/`Role` by name and `frust::accesskit` is the whole-crate
/// valve, so a bare `accesskit::` here means a consumer re-declared a direct
/// dependency on a crate whose version is pinned in exactly one place
/// (`docs/DEVELOPMENT.md` § Version-Pin Policy, `accesskit 0.24`) — precisely
/// the version-forgeability hole this seam exists to close.
const NO_VALVE_CRATES: &[&str] = &["frust_core", "frust_scene", "frust_text", "accesskit"];

/// The two crates with a sanctioned long-tail escape valve —
/// `frust::kurbo::X`/`frust::peniko::X` — for types `authoring` does not lift
/// by name. A bare reference is a violation ONLY when it is neither
/// same-line-prefixed by `frust::` nor part of an active multi-line
/// `use frust::{ ... }` group (see this module's "The `frust::`-prefixed
/// valve" doc).
const VALVED_CRATES: &[&str] = &["kurbo", "peniko"];

/// Every byte offset in `line` where `<crate_name>::` starts a genuine
/// top-level crate-path reference — bounded on the left by a non-identifier
/// character (or start of line) so a search for `kurbo` doesn't
/// false-positive inside a longer identifier (e.g. a hypothetical
/// `my_kurbo::Thing`). The literal `::` immediately after already
/// disambiguates the right side: no Rust identifier can contain it.
fn bare_crate_path_positions(line: &str, crate_name: &str) -> Vec<usize> {
    let needle = format!("{crate_name}::");
    let mut positions = Vec::new();
    let mut search_from = 0;
    while let Some(found_at) = line[search_from..].find(needle.as_str()) {
        let idx = search_from + found_at;
        let before = if idx == 0 {
            None
        } else {
            line[..idx].chars().next_back()
        };
        let boundary_ok = before.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        if boundary_ok {
            positions.push(idx);
        }
        search_from = idx + needle.len();
    }
    positions
}

/// True if the crate-path reference starting at byte offset `idx` in `line`
/// is immediately preceded by `frust::` on the same line — i.e. it is the
/// SAME-LINE half of the `frust::kurbo`/`frust::peniko` valve
/// (`frust::kurbo::Circle`, `frust::peniko::color::DynamicColor`), not a bare
/// top-level reference.
fn same_line_frust_prefixed(line: &str, idx: usize) -> bool {
    line[..idx].ends_with("frust::")
}

/// True if `trimmed` (a line already trimmed of leading whitespace) opens a
/// `use frust::` import — either a single-line statement or the first line of
/// a multi-line brace group.
fn starts_frust_use(trimmed: &str) -> bool {
    trimmed.starts_with("use frust::")
}

/// All violations found in `contents` (one consumer source file's full text),
/// formatted `path:line: <line>` — the offending file, line, and (via the
/// message this feeds into) the `frust::authoring` path to use instead.
///
/// Implements, in order, every exclusion this module's doc comment names:
/// comment-only lines, `#[cfg(test)]` modules (brace-depth-skipped from the
/// attribute's next line to its matching close), and — for `kurbo`/`peniko`
/// only — the `frust::`-prefixed valve, tracked across multi-line
/// `use frust::{ ... }` groups exactly like `surface_mode_conformance.rs`
/// tracks a multi-line `pub use` group.
fn violations_in(path: &Path, contents: &str) -> Vec<String> {
    let mut failures = Vec::new();

    // `#[cfg(test)]` module skip: every real `#[cfg(test)]` in this migrated
    // tree is immediately followed by `mod ... {` (verified against the
    // actual tree — see this module's doc comment for the three fixture
    // sites); once inside, brace depth tracks back down to the line that
    // closes the module, and every line in between is skipped entirely.
    let mut pending_cfg_test = false;
    let mut skip_until_depth: Option<i32> = None;
    let mut depth: i32 = 0;

    // Multi-line `use frust::{ ... };` group tracking, mirroring
    // `surface_mode_conformance.rs`'s grouped-`pub use` brace tracking.
    let mut in_frust_use = false;
    let mut frust_use_depth: i32 = 0;

    for (i, line) in contents.lines().enumerate() {
        let trimmed = line.trim_start();

        if let Some(target_depth) = skip_until_depth {
            depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
            if depth <= target_depth {
                skip_until_depth = None;
            }
            continue;
        }

        if is_comment_only(line) {
            continue;
        }

        if trimmed.starts_with("#[cfg(test)]") {
            pending_cfg_test = true;
            continue;
        }
        if pending_cfg_test {
            pending_cfg_test = false;
            // Tolerate a `pub`/`pub(crate)` prefix: `#[cfg(test)] pub mod tests {`
            // is as legitimate a shape as the bare `mod tests {` every current
            // consumer happens to use, and silently failing to recognise it
            // would turn dev-only code into spurious violations.
            let after_vis = trimmed
                .strip_prefix("pub(crate) ")
                .or_else(|| trimmed.strip_prefix("pub "))
                .unwrap_or(trimmed);
            if after_vis.starts_with("mod ") {
                let opens = line.matches('{').count() as i32;
                let closes = line.matches('}').count() as i32;
                // ONLY enter skip mode when the line actually opens a block.
                // An out-of-line `#[cfg(test)] mod tests;` (or a one-line
                // `mod tests { }`) opens nothing, and arming the skip there
                // would consume exactly one following production line
                // unscanned — a silent false negative, the failure mode this
                // scan exists to prevent.
                if opens > closes {
                    skip_until_depth = Some(depth);
                    depth += opens - closes;
                    continue;
                }
            }
        }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;

        // Track whether THIS line is part of an active `use frust::{ ... }`
        // group before running the valved-crate check below, so the group's
        // opening line and every continuation line (including one that
        // closes it) are all treated as "in group".
        if !in_frust_use && starts_frust_use(trimmed) {
            in_frust_use = true;
            frust_use_depth = 0;
        }
        let this_line_in_frust_use = in_frust_use;
        if in_frust_use {
            frust_use_depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
            if frust_use_depth <= 0 && line.contains(';') {
                in_frust_use = false;
                frust_use_depth = 0;
            }
        }

        for crate_name in NO_VALVE_CRATES {
            if !bare_crate_path_positions(line, crate_name).is_empty() {
                failures.push(format!(
                    "{}:{}: {}  —  use `frust::authoring` (or its `text`/`scene` submodules) \
                     instead of naming `{crate_name}` directly",
                    rel(path),
                    i + 1,
                    line.trim(),
                ));
            }
        }

        for crate_name in VALVED_CRATES {
            for idx in bare_crate_path_positions(line, crate_name) {
                if same_line_frust_prefixed(line, idx) || this_line_in_frust_use {
                    continue;
                }
                let authoring_hint = if *crate_name == "kurbo" {
                    "`frust::authoring` (for a by-name-lifted type: Affine, BezPath, Line, \
                     Point, Rect, RoundedRect, Shape, Size, Stroke, Vec2) or the long-tail \
                     valve `frust::kurbo`"
                } else {
                    "`frust::authoring` (for a by-name-lifted type: Brush, Color, Fill, \
                     ImageData) or the long-tail valve `frust::peniko`"
                };
                failures.push(format!(
                    "{}:{}: {}  —  use {authoring_hint} instead of naming `{crate_name}` directly",
                    rel(path),
                    i + 1,
                    line.trim(),
                ));
            }
        }
    }

    failures
}

#[test]
fn migrated_consumers_never_bypass_the_authoring_seam() {
    let mut failures = Vec::new();
    for path in consumer_source_files() {
        let contents =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        failures.extend(violations_in(&path, &contents));
    }
    assert!(
        failures.is_empty(),
        "a migrated production consumer named a facade-internal crate directly instead of going \
         through `frust::authoring` ({} hit(s)) — see this test's own module docs and this \
         feature's `PLAN.md`:\n{}",
        failures.len(),
        failures.join("\n"),
    );
}

/// Fixture-driven unit tests for the scan's trickiest cases: the exact three
/// `#[cfg(test)]` sites this module's doc comment cites as the tree's real
/// fixtures, plus the multi-line `use frust::{ ... }` valve group and the
/// same-line `frust::kurbo`/`frust::peniko` valve.
#[cfg(test)]
mod scan_behavior {
    use super::*;
    use std::path::Path;

    fn scan(src: &str) -> Vec<String> {
        violations_in(Path::new("fixture.rs"), src)
    }

    #[test]
    fn cfg_test_module_is_fully_skipped() {
        // Mirrors `examples/huddle/src/ui/sheet.rs` (~line 973) verbatim in
        // shape: a `#[cfg(test)] mod tests { ... }` block naming
        // `frust_core`/`kurbo` inline, which must produce zero violations.
        let src = "\
fn production_code() {}

#[cfg(test)]
mod tests {
    use frust_core::{BuildCtx, LayoutCtx, PaintCtx, RenderRoot, WindowEdgeInsets, WindowInsets};
    use kurbo::Size;

    fn fill_path(_path: &kurbo::BezPath, _brush: &peniko::Brush) {}
}
";
        assert!(
            scan(src).is_empty(),
            "a #[cfg(test)] module must be fully excluded from the scan"
        );
    }

    #[test]
    fn cfg_test_module_toast_shape_is_skipped() {
        // Mirrors `examples/huddle/src/ui/toast.rs` (~line 372).
        let src = "\
#[cfg(test)]
mod tests {
    use frust_core::{FrameTime, RenderRoot};
    use peniko::Color;
}
";
        assert!(scan(src).is_empty());
    }

    #[test]
    fn cfg_test_module_glyph_catalog_interactions_shape_is_skipped() {
        // Mirrors `examples/glyph-catalog/src/pages/interactions.rs`
        // (~line 1455), including its doc-comment-bearing module and a
        // second nested `use` block deeper in the same module (the
        // `kurbo::{Affine, Shape}` sub-scope at ~line 1632).
        let src = "\
#[cfg(test)]
mod tests {
    //! Headless test doc.
    use frust_scene::GlyphRun;
    use frust_text::TextContext;
    use kurbo::{BezPath, Point, Rect, Size};
    use peniko::{Brush, Color};

    fn nested() {
        use kurbo::{Affine, Shape};
    }
}
";
        assert!(
            scan(src).is_empty(),
            "the whole cfg(test) module, including a nested inner scope, must be skipped"
        );
    }

    #[test]
    fn production_reference_outside_cfg_test_is_flagged() {
        let src = "use frust_core::Widget;\n";
        let failures = scan(src);
        assert_eq!(
            failures.len(),
            1,
            "a production bare `frust_core::` must be flagged"
        );
    }

    #[test]
    fn same_line_frust_prefixed_valve_is_not_flagged() {
        let src = "\
use frust::kurbo::Circle;
use frust::peniko::color::DynamicColor;
use frust::peniko::{ColorStop, Gradient, GradientKind};
";
        assert!(
            scan(src).is_empty(),
            "`frust::kurbo`/`frust::peniko` is the sanctioned long-tail valve, not a violation"
        );
    }

    #[test]
    fn multi_line_frust_use_group_valve_is_not_flagged() {
        // Mirrors `examples/huddle/src/features/messages/presentation/pages/
        // thread.rs`'s real shape: `kurbo::Size` sits on its own line inside a
        // multi-item `use frust::{ ... };` group, with no `frust::` literally
        // on that same line.
        let src = "\
use frust::{
    Align, Alignment, AnyView, Axis, Color, CrossAxisAlignment, EdgeInsets, FlexView,
    GestureDetector, Get, GetUntracked, Padding, RwSignal, Set, SizedBox, Stack, any, app_bar,
    hero, icon, icons, inflexible, keyed, kurbo::Size, scroll_view, text, text_input, use_context,
};
";
        assert!(
            scan(src).is_empty(),
            "a `kurbo::`/`peniko::` sub-item split across lines inside a `use frust::{{ ... }}` \
             group is the same valve reference as the same-line form, not a violation"
        );
    }

    #[test]
    fn bare_kurbo_outside_any_frust_context_is_flagged() {
        let src = "use kurbo::Point;\n";
        let failures = scan(src);
        assert_eq!(
            failures.len(),
            1,
            "a bare top-level `kurbo::` import with no `frust::` valve anywhere must be flagged"
        );
    }

    #[test]
    fn comment_only_mention_is_not_flagged() {
        // Mirrors `examples/glyph-catalog/src/pages/navigation.rs`'s module
        // doc, which discusses `frust-core`/`kurbo`/`peniko` in prose.
        let src = "\
//! Huddle's `Tab::glyph_icon` builds its vector icons from a
//! `kurbo::BezPath` directly — but huddle carries a documented, one-off
//! `frust-core`/`kurbo`/`peniko` escape-hatch dependency.
";
        assert!(
            scan(src).is_empty(),
            "a doc-comment mention in prose must never be flagged"
        );
    }

    #[test]
    fn bare_crate_path_boundary_does_not_false_positive_on_a_longer_identifier() {
        assert!(
            bare_crate_path_positions("let x = my_kurbo::Thing;", "kurbo").is_empty(),
            "`my_kurbo::` must not be mistaken for a bare `kurbo::` reference"
        );
        assert_eq!(
            bare_crate_path_positions("let x = kurbo::Point::ZERO;", "kurbo").len(),
            1,
            "the exact identifier `kurbo::` must still be detected"
        );
    }
}
