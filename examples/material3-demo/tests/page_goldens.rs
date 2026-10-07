//! This gallery's own page goldens: four representative Material 3 Expressive
//! screens captured GPU-free and compared against the framework's committed
//! `cpu/` golden class.
//!
//! ```text
//! (cd examples/material3-demo && cargo test --test page_goldens)
//! (cd examples/material3-demo && UPDATE_GOLDENS=1 cargo test --test page_goldens)
//! ```
//!
//! # Why these pages live here and not in `frust-testing`
//!
//! This package is EXCLUDED from the root workspace (see `Cargo.toml`'s
//! header: its generated Android/iOS projects need a project-local `target/`,
//! and its `[profile.*]` blocks only take effect in a workspace root), so the
//! root graph cannot import it. `frust-testing`'s own `corpus::page` therefore
//! reconstructs pages from the four root-member catalogs and stops there; this
//! file is the gallery's half of that split, reaching the identical pipeline
//! through a path `[dev-dependencies]` edge on `frust-testing` — the only edge
//! shape that crate's `tests/deps.rs` guard allows anyone to declare.
//!
//! The baselines still land in the framework's one corpus
//! (`testing/goldens/cpu/`, resolved from `frust-testing`'s own manifest
//! directory), under `page-m3demo-*` stems so they cannot collide with the
//! root corpus's `page-*`/`widget-*`/`unit-*` files.
//!
//! # Why the pages are rebuilt rather than imported
//!
//! `tests/widget_test.rs`'s choice, for its reason: this crate's `catalog`,
//! `pages` and `theme` modules are private, and widening them to `pub` purely
//! so a test could observe them would export app internals as API. Each screen
//! below is rebuilt from `frust_material`'s public constructors in the shape
//! `src/lib.rs` composes them — a scaffold with a top app bar, a section body
//! and a bottom navigation bar; a playground of one component's variants — so
//! what the baselines pin is the catalog's rendering, which is what this
//! gallery exists to gate.
//!
//! # Determinism
//!
//! Every input is fixed: a 412x892 logical viewport at scale 2.0, the Material
//! baseline theme with its type scale pinned to `frust-testing`'s bundled
//! faces, fixed selection/progress values, and `FrameTime::ZERO`. Text this
//! file authors is pinned to a bundled face. The app bar title and the nav
//! items' labels resolve their family from that pinned type scale, so they
//! would be portable too; they are passed empty only because the committed
//! baselines were captured that way. A run that shaped against a host font
//! would make the baseline runner-local — `frust_testing::foreign_font_runs` is
//! the gate that enforces it, and [`no_page_shapes_against_a_host_font`] runs
//! it here directly.

use frust::authoring::text::FontFamily;
use frust::{AnyView, Color, CrossAxisAlignment, View, any};
use frust_material as m3;
use frust_testing::case::Tolerance;
use frust_testing::corpus::CorpusCase;
use frust_testing::frame::{
    FrameSpec, SAMPLE_TEXT, SAMPLE_TEXT_LONG, foreign_font_runs, glyph_run_count, physical_case,
    pin_type_scale, pinned_text_style, record_view, test_text_context,
};
use frust_testing::{Scene, run_cpu_goldens};

/// Ink for text this file authors — near-black on the gallery's light
/// surfaces.
const INK: Color = Color::from_rgb8(0x14, 0x14, 0x18);

/// Outer gutter each page lays its content inside, in LOGICAL px.
const GUTTER: f64 = 20.0;

/// Vertical gap between stacked sections, in LOGICAL px.
const GAP: f64 = 14.0;

/// This gallery's page cases, in declaration order.
fn m3demo_cases() -> Vec<CorpusCase> {
    vec![
        gallery_shell(),
        buttons(),
        selection(),
        progress_and_cards(),
    ]
}

/// The case shape every page here takes: a phone frame at `frust-testing`'s
/// tight default tolerance (channel 2, alpha 2, zero tolerated mismatched
/// pixels).
fn case(name: &'static str) -> frust_testing::CaseSpec {
    physical_case(name, &FrameSpec::phone(), Tolerance::new())
}

/// A vertical gap of [`GAP`] logical px.
fn gap() -> impl View<()> {
    frust::SizedBox::<()>(None, Some(GAP))
}

/// A horizontal gap of [`GAP`] logical px.
fn hgap() -> impl View<()> {
    frust::SizedBox::<()>(Some(GAP), None)
}

/// Records one still frame of `build` under the Material baseline theme, with
/// that theme's type scale pinned to the bundled faces.
///
/// `frust_testing::frame::record_view` owns the whole capture — the
/// `RenderRoot`, the shell-order rebuild/layout/paint and the paint-time device
/// scale — so this file never names a `frust-core` type and needs no second
/// path dependency to reach one.
fn record_page(scene: &mut Scene, build: impl Fn(&FontFamily) -> AnyView<()> + 'static) {
    let (mut tcx, family) = test_text_context();
    let theme = pin_type_scale(m3::baseline(), &family);
    record_view(scene, &FrameSpec::phone(), theme, &mut tcx, move || {
        build(&family)
    });
}

/// Text this file authors, pinned to the bundled Latin face.
fn label(family: &FontFamily, content: &str, size: f32) -> impl View<()> {
    frust::text(content).style(pinned_text_style(family, size, INK))
}

/// The gallery shell: the top app bar, a section list of tappable cards, and
/// the bottom navigation bar — `src/lib.rs`'s own composition, rebuilt from
/// `frust_material`'s public constructors.
fn gallery_shell() -> CorpusCase {
    fn row(family: &FontFamily) -> impl View<()> {
        m3::filled_card::<(), _>(frust::Padding(
            frust::EdgeInsets::all(16.0),
            label(family, SAMPLE_TEXT, 17.0),
        ))
    }
    fn record(scene: &mut Scene) {
        record_page(scene, |family| {
            any(frust::scaffold::<(), _>(frust::Padding(
                frust::EdgeInsets::all(GUTTER),
                frust::column()
                    .child(row(family))
                    .child(gap())
                    .child(row(family))
                    .child(gap())
                    .child(row(family))
                    .child(gap())
                    .child(row(family))
                    .cross_axis(CrossAxisAlignment::Stretch),
            ))
            .app_bar(m3::app_bar::<()>(""))
            .bottom_bar(m3::navigation_bar::<(), _>(
                vec![
                    m3::nav_item::<()>(""),
                    m3::nav_item::<()>(""),
                    m3::nav_item::<()>(""),
                    m3::nav_item::<()>(""),
                    m3::nav_item::<()>(""),
                ],
                0,
                |_: &mut (), _: usize| {},
            )))
        });
    }
    CorpusCase {
        spec: case("page-m3demo-gallery"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "the gallery shell: top app bar, a section card list, and the 5-tab \
                navigation bar",
    }
}

/// The Buttons playground: every `frust_material` button factory at rest, plus
/// the disabled arm.
///
/// These labels are real. A Material button resolves its label style from
/// `Theme::type_scale.label_*`, which `pin_type_scale` rewrites onto a bundled
/// face, so this text is portable.
fn buttons() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_page(scene, |family| {
            any(frust::container::<(), _>(frust::Padding(
                frust::EdgeInsets::all(GUTTER),
                frust::column()
                    .child(label(family, SAMPLE_TEXT, 22.0))
                    .child(gap())
                    .child(
                        frust::row()
                            .child(m3::filled_button::<(), _>(SAMPLE_TEXT, |_: &mut ()| {}))
                            .child(hgap())
                            .child(m3::tonal_button::<(), _>(SAMPLE_TEXT, |_: &mut ()| {})),
                    )
                    .child(gap())
                    .child(
                        frust::row()
                            .child(m3::elevated_button::<(), _>(SAMPLE_TEXT, |_: &mut ()| {}))
                            .child(hgap())
                            .child(m3::outlined_button::<(), _>(SAMPLE_TEXT, |_: &mut ()| {})),
                    )
                    .child(gap())
                    .child(m3::text_button::<(), _>(SAMPLE_TEXT, |_: &mut ()| {}))
                    .child(gap())
                    .child(m3::filled_button::<(), _>(SAMPLE_TEXT, |_: &mut ()| {}).enabled(false))
                    .child(gap())
                    .child(m3::divider())
                    .cross_axis(CrossAxisAlignment::Stretch),
            ))
            .fill(Color::WHITE)
            .expand())
        });
    }
    CorpusCase {
        spec: case("page-m3demo-buttons"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "the Buttons playground: filled/tonal/elevated/outlined/text at rest, \
                plus the disabled arm",
    }
}

/// The selection playground: switch and checkbox in both of their states, and
/// two slider positions.
///
/// None of these controls carries a label: a switch and a checkbox have no
/// label API, and a slider's value-indicator label (whose style already comes
/// from `Theme::type_scale.label_large`) paints only while the thumb is
/// pressed. The control geometry is what this page pins.
fn selection() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_page(scene, |family| {
            any(frust::container::<(), _>(frust::Padding(
                frust::EdgeInsets::all(GUTTER),
                frust::column()
                    .child(label(family, SAMPLE_TEXT, 22.0))
                    .child(gap())
                    .child(
                        frust::row()
                            .child(m3::switch::<(), _>(true, |_: &mut (), _: bool| {}))
                            .child(hgap())
                            .child(m3::switch::<(), _>(false, |_: &mut (), _: bool| {})),
                    )
                    .child(gap())
                    .child(
                        frust::row()
                            .child(m3::checkbox::<(), _>(true, |_: &mut (), _: bool| {}))
                            .child(hgap())
                            .child(m3::checkbox::<(), _>(false, |_: &mut (), _: bool| {})),
                    )
                    .child(gap())
                    .child(m3::slider::<(), _>(0.35, |_: &mut (), _: f64| {}))
                    .child(gap())
                    .child(m3::slider::<(), _>(0.8, |_: &mut (), _: f64| {}))
                    .child(gap())
                    .child(label(family, SAMPLE_TEXT_LONG, 13.0))
                    .cross_axis(CrossAxisAlignment::Stretch),
            ))
            .fill(Color::WHITE)
            .expand())
        });
    }
    CorpusCase {
        spec: case("page-m3demo-selection"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "the selection playground: switch/checkbox in both states and two slider \
                positions",
    }
}

/// A progress-and-surfaces page: the linear and circular indicators at fixed
/// determinate values over the three card variants.
fn progress_and_cards() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_page(scene, |family| {
            fn card(view: impl View<()>) -> impl View<()> {
                m3::outlined_card::<(), _>(frust::Padding(frust::EdgeInsets::all(16.0), view))
            }
            any(frust::container::<(), _>(frust::Padding(
                frust::EdgeInsets::all(GUTTER),
                frust::column()
                    .child(label(family, SAMPLE_TEXT, 22.0))
                    .child(gap())
                    .child(card(m3::linear_progress(m3::ProgressValue::Determinate(
                        0.25,
                    ))))
                    .child(gap())
                    .child(card(m3::linear_progress(m3::ProgressValue::Determinate(
                        0.75,
                    ))))
                    .child(gap())
                    .child(card(m3::circular_progress(m3::ProgressValue::Determinate(
                        0.4,
                    ))))
                    .child(gap())
                    .child(m3::elevated_card::<(), _>(frust::Padding(
                        frust::EdgeInsets::all(16.0),
                        label(family, SAMPLE_TEXT, 16.0),
                    )))
                    .cross_axis(CrossAxisAlignment::Stretch),
            ))
            .fill(Color::WHITE)
            .expand())
        });
    }
    CorpusCase {
        spec: case("page-m3demo-progress"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "determinate linear and circular progress inside outlined and elevated cards",
    }
}

/// The gate: all four pages against their committed `cpu/` baselines.
#[test]
fn the_gallery_pages_match_their_goldens() {
    let cases = m3demo_cases();
    let report = run_cpu_goldens(&cases);
    println!(
        "page_goldens: material3-demo — {} compared, {} promoted",
        report.compared, report.promoted
    );
    report.assert_passed("the material3-demo page corpus");
}

/// No page here shapes a glyph against a host font.
///
/// Stated separately from the golden gate for the same reason the framework's
/// own harness states it separately: a font regression should read as a font
/// regression, and this check is meaningful before any baseline exists.
#[test]
fn no_page_shapes_against_a_host_font() {
    let mut failures = Vec::new();
    for case in m3demo_cases() {
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

/// Naming and framing: `page-m3demo-*` stems, unique, one full phone frame
/// each, with the device scale applied exactly once.
#[test]
fn every_case_is_a_uniquely_named_phone_frame() {
    let mut seen = std::collections::BTreeSet::new();
    for case in m3demo_cases() {
        assert!(
            case.spec.name.starts_with("page-m3demo-"),
            "`{}` must carry the page-m3demo- stem so it cannot collide with the root \
             corpus's own files in testing/goldens/cpu/",
            case.spec.name
        );
        assert!(
            seen.insert(case.spec.name),
            "duplicate case name `{}`",
            case.spec.name
        );
        assert_eq!(case.spec.width, 824, "{}", case.spec.name);
        assert_eq!(case.spec.height, 1784, "{}", case.spec.name);
        assert_eq!(
            case.spec.scale, 1.0,
            "`{}` would apply the device scale twice — `frame()` already pushed it",
            case.spec.name
        );
    }
}

/// Every page records real commands and shapes real text — the tripwire that
/// says a page is a page and not a plausible blank frame.
#[test]
fn every_page_records_commands_and_shapes_text() {
    for case in m3demo_cases() {
        let scene = case.scene();
        assert!(
            !scene.commands().is_empty(),
            "`{}` recorded no commands",
            case.spec.name
        );
        assert!(
            glyph_run_count(&scene) > 0,
            "`{}` shaped no text",
            case.spec.name
        );
    }
}
