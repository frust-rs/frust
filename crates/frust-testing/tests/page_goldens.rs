//! The widget/page corpus's golden gate: every
//! [`frust_testing::corpus::widget`] and [`frust_testing::corpus::page`] case
//! rendered through a real `RenderRoot` and compared against the committed
//! `cpu/` baselines.
//!
//! ```text
//! cargo test -p frust-testing --test page_goldens
//! UPDATE_GOLDENS=1 cargo test -p frust-testing --test page_goldens
//! ```
//!
//! # Why there is no GPU arm here
//!
//! `tests/goldens.rs` runs the unit corpus on both arms of the oracle pair,
//! because a `frust_scene::Command`'s lowering is exactly what differs between
//! a CPU rasterizer and vello-on-wgpu. These cases are about what the WIDGET
//! LAYER emits — the display list a real tree produces at a fixed size, scale,
//! theme and frame time — and that is GPU-free by construction: the scene is
//! captured from `RenderRoot::paint`, and the CPU oracle is the only renderer
//! involved. A GPU arm here would re-test `tests/goldens.rs`'s subject through
//! a much larger frame.
//!
//! # What each case is gated on
//!
//! [`frust_testing::run_cpu_goldens`] is the shared runner (see its docs for
//! the order): font determinism first, then the oracle's fidelity report, then
//! the case's probes, then the baseline. `examples/material3-demo` runs its own
//! pages through the same function from its own workspace — that crate is
//! excluded from the root graph and cannot be imported here, so the pipeline
//! lives in the library rather than in this file.

use std::collections::BTreeSet;

use frust_testing::corpus::{page_cases, widget_cases};
use frust_testing::frame::{foreign_font_runs, glyph_run_count};
use frust_testing::golden::goldens_root;
use frust_testing::run_cpu_goldens;

/// The whole widget corpus against its committed `cpu/` baselines.
#[test]
fn the_widget_corpus_matches_its_goldens() {
    let cases = widget_cases();
    let report = run_cpu_goldens(&cases);
    println!(
        "page_goldens: widget — {} compared, {} promoted",
        report.compared, report.promoted
    );
    report.assert_passed("the widget corpus");
}

/// The whole page corpus against its committed `cpu/` baselines.
#[test]
fn the_page_corpus_matches_its_goldens() {
    let cases = page_cases();
    let report = run_cpu_goldens(&cases);
    println!(
        "page_goldens: page — {} compared, {} promoted",
        report.compared, report.promoted
    );
    report.assert_passed("the page corpus");
}

/// No case in either corpus shapes a glyph against a host font.
///
/// [`run_cpu_goldens`] already refuses such a case, but it refuses it as one
/// failure among many in a golden run. This is the same property stated on its
/// own, so a font regression reads as a font regression rather than as a
/// baseline mismatch — and it runs with no baseline present at all, which is
/// what makes it useful the first time a case is written.
#[test]
fn no_case_shapes_against_a_host_font() {
    let mut failures = Vec::new();
    for case in widget_cases().into_iter().chain(page_cases()) {
        for message in foreign_font_runs(&case.scene()) {
            failures.push(format!("[{}] {message}", case.spec.name));
        }
    }
    assert!(
        failures.is_empty(),
        "{} non-portable frame(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Golden-file naming: widget cases are `widget-*`, page cases `page-*`, and
/// no two cases in the corpus share a name.
///
/// The prefixes are a directory contract, not a style preference — the whole
/// corpus shares one flat `testing/goldens/cpu/` directory with the unit
/// corpus's `unit-*` files and with a sibling card's `adv-*` files, so a stem
/// collision would silently overwrite someone else's baseline.
#[test]
fn every_case_name_is_unique_and_correctly_prefixed() {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (prefix, cases) in [("widget-", widget_cases()), ("page-", page_cases())] {
        for case in cases {
            assert!(
                case.spec.name.starts_with(prefix),
                "`{}` must start with `{prefix}`",
                case.spec.name
            );
            assert!(
                seen.insert(case.spec.name),
                "duplicate case name `{}`",
                case.spec.name
            );
        }
    }
}

/// Every case in both corpora is one full phone frame: 824x1784 physical, and
/// no second scale.
///
/// The scale assertion is the one that would fail silently otherwise — a case
/// that set `CaseSpec::scale` as well as letting `frame()` push the root
/// transform would render at 4x and still produce a plausible-looking image.
#[test]
fn every_case_is_a_phone_frame_with_the_scale_applied_exactly_once() {
    for case in widget_cases().into_iter().chain(page_cases()) {
        assert_eq!(case.spec.width, 824, "{}", case.spec.name);
        assert_eq!(case.spec.height, 1784, "{}", case.spec.name);
        assert_eq!(
            case.spec.scale, 1.0,
            "`{}` would apply the device scale twice",
            case.spec.name
        );
    }
}

/// Every page case shapes real text, and every case records real commands.
///
/// A page whose widgets all failed to build would still render a plausible
/// blank frame and freeze it as a baseline; this is the tripwire that says a
/// page is actually a page.
#[test]
fn every_page_records_commands_and_shapes_text() {
    for case in page_cases() {
        let scene = case.scene();
        assert!(
            !scene.commands().is_empty(),
            "`{}` recorded no commands",
            case.spec.name
        );
        assert!(
            glyph_run_count(&scene) > 0,
            "`{}` is a catalog page and shaped no text",
            case.spec.name
        );
    }
}

/// Every case that this gate compares has a committed baseline PNG and its
/// metadata sidecar.
///
/// `cpu/` is the baseline-REQUIRED class (`tests/goldens.rs`'s module docs):
/// a missing file is a real failure on any machine, not a "not reviewed here
/// yet". Checked as a directory listing so the message names the missing file
/// rather than surfacing as a comparison error.
#[test]
fn every_case_has_a_committed_cpu_baseline() {
    let root = goldens_root().join("cpu");
    let mut missing = Vec::new();
    for case in widget_cases().into_iter().chain(page_cases()) {
        for extension in ["png", "json"] {
            let path = root.join(format!("{}.{extension}", case.spec.name));
            if !path.is_file() {
                missing.push(path.display().to_string());
            }
        }
    }
    assert!(
        missing.is_empty(),
        "{} missing `cpu/` baseline file(s) — promote with `UPDATE_GOLDENS=1 cargo test \
         -p frust-testing --test page_goldens`:\n{}",
        missing.len(),
        missing.join("\n")
    );
}
