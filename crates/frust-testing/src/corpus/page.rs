//! The PAGE corpus: representative catalog screens built from the four
//! design-system plugin crates, each captured as one GPU-free frame.
//!
//! # What this corpus is for
//!
//! [`super::unit`] pins one scene command; [`super::widget`] pins one baseline
//! widget's state. These cases pin a whole SCREEN: a design system's own
//! theme, its chrome (app/nav/tab bars), its surfaces (cards, dialogs) and its
//! controls, composed the way a catalog page composes them and rendered
//! through a real [`frust_core::RenderRoot`]. That is the level at which a
//! token change, a shape-scale change or a container-geometry change actually
//! shows up.
//!
//! Two pages per design system — a SCREEN (chrome plus content) and a
//! CONTROLS/surfaces page — from the four root-workspace catalogs:
//! `plugins/{material,cupertino,glyph,shadcn}`. `examples/material3-demo` is
//! deliberately absent: it is an EXCLUDED workspace, unimportable from the
//! root graph, so it carries its own `tests/page_goldens.rs` over a path
//! dev-dependency on this crate and runs through the same
//! [`crate::frame::run_cpu_goldens`].
//!
//! # Deterministic state
//!
//! Every page is a pure function of its inputs: a fixed 412x892 logical
//! viewport at scale 2.0, a fixed theme (the catalog's own baseline, with its
//! type scale pinned — see below), fixed selection/checked/progress values,
//! and a fixed [`frust_core::FrameTime`]. Nothing consults a clock, so an
//! animating widget on one of these pages (a progress track, an activity
//! indicator, a state-layer ramp) resolves to the same phase on every run.
//!
//! # Text
//!
//! Same rule as [`super::widget`], and the same gate
//! ([`crate::frame::foreign_font_runs`]): every glyph in a committed baseline
//! must come from a bundled face.
//!
//! The catalogs split cleanly on whether a caller can achieve that. Material's
//! buttons resolve their label style from `Theme::type_scale.label_*`, so
//! [`crate::frame::pin_type_scale`] reaches them and they carry real labels
//! here. Most other catalog chrome — a Material app bar's title, a Cupertino
//! nav bar's title, a shadcn `card_title`, a Glyph tag's label — hardcodes its
//! size/weight and takes only its COLOR from the theme, leaving its FAMILY at
//! `TextStyle::default()` (`frust_text::FontFamily::SystemUi`). Those slots
//! are passed an EMPTY string, and the page's real text is supplied as a child
//! VIEW with a pinned style through the slots that take one
//! (`glyph_card().title(..)`, `card_content(..)`, a card's child, a dialog's
//! actions). What the baselines then pin is the chrome's geometry and the
//! catalog's own tokens, which is a page case's subject anyway; the
//! framework-side fix — a label style/child seam on the string-slot widgets —
//! is a catalog/`frust-widgets` change, not something a corpus can work
//! around.

use frust_core::{AnyView, RenderRoot, any};
use frust_scene::Scene;
use frust_text::FontFamily;
use frust_theme::Theme;
use frust_widgets::{
    Column, CrossAxisAlignment, EdgeInsets, Padding, Row, SizedBox, colored_box, container, text,
};
use peniko::Color;

use super::CorpusCase;
use crate::case::{CaseSpec, Tolerance};
use crate::frame::{
    FrameSpec, SAMPLE_TEXT, SAMPLE_TEXT_LONG, frame, physical_case, pin_type_scale,
    pinned_text_style, test_text_context,
};

/// Every page case's `()`-state root view type.
type Root = RenderRoot<(), AnyView<()>>;

/// Ink for page-authored text — near-black, legible on a light catalog
/// surface without being a pure-black cliff.
const INK: Color = Color::from_rgb8(0x14, 0x14, 0x18);

/// Ink for page-authored text on the Glyph catalog's dark surfaces.
const INK_ON_DARK: Color = Color::from_rgb8(0xE6, 0xEA, 0xF2);

/// Outer gutter every page lays its content inside, in LOGICAL px.
const GUTTER: f64 = 20.0;

/// Vertical gap between a page's stacked sections, in LOGICAL px.
const GAP: f64 = 14.0;

/// The Glyph catalog's page background — its own surfaces are dark, and a
/// page case renders on the background its catalog actually ships.
const GLYPH_BG: Color = Color::from_rgb8(0x0B, 0x0D, 0x12);

/// The iOS grouped-list background a Cupertino settings screen sits on.
const IOS_GROUPED_BG: Color = Color::from_rgb8(0xF2, 0xF2, 0xF7);

/// The iOS separator colour between grouped rows.
const IOS_SEPARATOR: Color = Color::from_rgb8(0xD8, 0xD8, 0xDC);

/// The scrim a modal page renders its dialog over.
const SCRIM: Color = Color::from_rgb8(0x2A, 0x2C, 0x33);

/// The whole page corpus, in declaration order.
#[must_use]
pub fn page_cases() -> Vec<CorpusCase> {
    vec![
        material_home(),
        material_dialog(),
        cupertino_settings(),
        cupertino_controls(),
        glyph_dashboard(),
        glyph_surfaces(),
        shadcn_form(),
        shadcn_controls(),
    ]
}

/// The case shape every page takes: a phone frame at the crate's tight
/// default tolerance (channel 2, alpha 2, zero tolerated mismatched pixels).
fn case(name: &'static str) -> CaseSpec {
    physical_case(name, &FrameSpec::phone(), Tolerance::new())
}

/// Page-authored text at `size` in `ink`, pinned to the bundled face.
fn label(family: &FontFamily, content: &str, size: f32, ink: Color) -> frust_widgets::TextView {
    text(content).style(pinned_text_style(family, size, ink))
}

/// Page-authored text on a light catalog surface.
fn dark_on_light(family: &FontFamily, content: &str, size: f32) -> frust_widgets::TextView {
    label(family, content, size, INK)
}

/// Page-authored text on the Glyph catalog's dark surfaces.
fn light_on_dark(family: &FontFamily, content: &str, size: f32) -> frust_widgets::TextView {
    label(family, content, size, INK_ON_DARK)
}

/// A vertical gap of [`GAP`] logical px.
fn gap() -> AnyView<()> {
    any(SizedBox::<()>(None, Some(GAP)))
}

/// A horizontal gap of [`GAP`] logical px.
fn hgap() -> AnyView<()> {
    any(SizedBox::<()>(Some(GAP), None))
}

/// A fixed-size rounded box in `fill` — the stand-in for a catalog's icon
/// slot, which would otherwise shape an icon-font glyph and so reach a
/// non-bundled face.
fn swatch(size: f64, fill: Color) -> AnyView<()> {
    any(colored_box::<()>().fill(fill).size(size, size).radius(6.0))
}

/// Records one still frame of `build` under `theme`, with the theme's type
/// scale pinned to the bundled face.
///
/// The design system's OWN baseline theme is the input — a page case that
/// substituted [`Theme::neutral`] would be pinning the widget layer again
/// rather than the catalog.
fn record_page(
    scene: &mut Scene,
    theme: impl Fn() -> Theme,
    build: impl Fn(&FontFamily) -> AnyView<()> + 'static,
) {
    let (mut tcx, family) = test_text_context();
    let spec = FrameSpec::phone();
    let mut root: Root = RenderRoot::new();
    root.set_theme(Box::new(pin_type_scale(theme(), &family)));
    let mut state = ();
    let mut logic = move |_: &mut ()| build(&family);
    frame(&mut root, &mut logic, &mut state, &mut tcx, &spec, scene);
}

/// Wraps `children` in the corpus's standard gutter-padded page body over
/// `background`.
///
/// The column is cross-axis STRETCHED, which is what makes these pages read as
/// pages: a catalog surface (a card, a separator, a progress track) spans the
/// content width on a real screen, and a start-aligned column would instead
/// shrink each one to its own natural width and pin geometry no shipped screen
/// has.
fn body(background: Color, children: Vec<AnyView<()>>) -> AnyView<()> {
    any(container::<(), _>(Padding(
        EdgeInsets::all(GUTTER),
        Column(children).cross_axis(CrossAxisAlignment::Stretch),
    ))
    .fill(background)
    .expand())
}

// ---------------------------------------------------------------------------
// Material
// ---------------------------------------------------------------------------

/// A Material home screen: app bar, three card variants over the catalog's own
/// surfaces, the four button variants (whose labels ARE pinnable — see the
/// module docs), a divider, a determinate progress track, a navigation bar and
/// a FAB, all inside a [`frust_widgets::scaffold`].
fn material_home() -> CorpusCase {
    /// A single-line card body. Deliberately one text run rather than a
    /// headline/supporting pair: this frame is 824x1784 physical pixels and
    /// `tests/corpus_budget.rs` caps a single golden PNG at 64 KB, which dense
    /// antialiased text reaches quickly — and the card VARIANT (fill, outline,
    /// elevation shadow) is what this page exists to pin, not its typography.
    fn card_body(family: &FontFamily) -> AnyView<()> {
        any(Padding::<(), _>(
            EdgeInsets::all(16.0),
            dark_on_light(family, SAMPLE_TEXT, 18.0),
        ))
    }
    fn record(scene: &mut Scene) {
        record_page(scene, frust_material::baseline, |family| {
            let content = Column(vec![
                any(frust_material::elevated_card::<(), _>(card_body(family))),
                gap(),
                any(frust_material::filled_card::<(), _>(card_body(family))),
                gap(),
                any(frust_material::outlined_card::<(), _>(card_body(family))),
                gap(),
                any(Row(vec![
                    any(frust_material::filled_button::<(), _>(
                        SAMPLE_TEXT,
                        |_: &mut ()| {},
                    )),
                    hgap(),
                    any(frust_material::tonal_button::<(), _>(
                        SAMPLE_TEXT,
                        |_: &mut ()| {},
                    )),
                ])),
                gap(),
                any(Row(vec![
                    any(frust_material::outlined_button::<(), _>(
                        SAMPLE_TEXT,
                        |_: &mut ()| {},
                    )),
                    hgap(),
                    any(frust_material::text_button::<(), _>(
                        SAMPLE_TEXT,
                        |_: &mut ()| {},
                    )),
                ])),
                gap(),
                any(frust_material::divider()),
                gap(),
                any(frust_material::linear_progress(
                    frust_material::ProgressValue::Determinate(0.62),
                )),
            ])
            .cross_axis(CrossAxisAlignment::Stretch);
            any(
                frust_widgets::scaffold::<(), _>(Padding(EdgeInsets::all(GUTTER), content))
                    .app_bar(any(frust_material::app_bar::<()>("")))
                    .bottom_bar(any(frust_material::navigation_bar::<(), _>(
                        vec![
                            frust_material::nav_item::<()>(""),
                            frust_material::nav_item::<()>(""),
                            frust_material::nav_item::<()>(""),
                        ],
                        1,
                        |_: &mut (), _: usize| {},
                    )))
                    .fab(any(frust_material::fab::<(), _>(
                        swatch(24.0, Color::WHITE),
                        |_: &mut ()| {},
                    ))),
            )
        });
    }
    CorpusCase {
        spec: case("page-material-home"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "a Material home screen: app bar, three card variants, four button variants, \
                divider, progress, navigation bar and FAB",
    }
}

/// A Material dialog with a two-button action row over a dimmed backdrop — the
/// modal surface, its elevation shadow and the action row's alignment.
fn material_dialog() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_page(scene, frust_material::baseline, |family| {
            let dialog = frust_material::dialog::<()>()
                .title("")
                .body("")
                .actions(vec![
                    any(frust_material::text_button::<(), _>(
                        SAMPLE_TEXT,
                        |_: &mut ()| {},
                    )),
                    any(frust_material::filled_button::<(), _>(
                        SAMPLE_TEXT,
                        |_: &mut ()| {},
                    )),
                ]);
            body(
                SCRIM,
                vec![
                    any(light_on_dark(family, SAMPLE_TEXT, 22.0)),
                    gap(),
                    any(light_on_dark(family, SAMPLE_TEXT_LONG, 14.0)),
                    gap(),
                    any(dialog),
                ],
            )
        });
    }
    CorpusCase {
        spec: case("page-material-dialog"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "a Material dialog surface with a two-button action row over a dimmed backdrop",
    }
}

// ---------------------------------------------------------------------------
// Cupertino
// ---------------------------------------------------------------------------

/// A Cupertino settings screen: nav bar, a grouped switch list on the iOS
/// grouped background, the activity indicator, and a tab bar.
fn cupertino_settings() -> CorpusCase {
    fn row(family: &FontFamily, checked: bool) -> AnyView<()> {
        any(container::<(), _>(Padding(
            EdgeInsets::symmetric(16.0, 12.0),
            Row(vec![
                any(dark_on_light(family, SAMPLE_TEXT, 17.0)),
                any(SizedBox::<()>(Some(120.0), None)),
                any(frust_cupertino::cupertino_switch::<(), _>(
                    checked,
                    |_: &mut (), _: bool| {},
                )),
            ]),
        ))
        .fill(Color::WHITE))
    }
    fn record(scene: &mut Scene) {
        record_page(scene, frust_cupertino::baseline, |family| {
            any(frust_widgets::scaffold::<(), _>(Padding(
                EdgeInsets::all(GUTTER),
                Column(vec![
                    any(dark_on_light(family, SAMPLE_TEXT, 13.0)),
                    gap(),
                    any(container::<(), _>(Column(vec![
                        row(family, true),
                        any(frust_widgets::divider(IOS_SEPARATOR)),
                        row(family, false),
                        any(frust_widgets::divider(IOS_SEPARATOR)),
                        row(family, true),
                    ]))
                    .radius(10.0)),
                    gap(),
                    any(frust_cupertino::cupertino_activity_indicator()),
                ]),
            ))
            .app_bar(any(frust_cupertino::cupertino_nav_bar::<()>("")))
            .bottom_bar(any(frust_cupertino::cupertino_tab_bar::<(), _>(
                vec![
                    frust_cupertino::tab_item::<()>(""),
                    frust_cupertino::tab_item::<()>(""),
                    frust_cupertino::tab_item::<()>(""),
                ],
                0,
                |_: &mut (), _: usize| {},
            )))
            .background(IOS_GROUPED_BG))
        });
    }
    CorpusCase {
        spec: case("page-cupertino-settings"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "a Cupertino settings screen: nav bar, a grouped switch list, and a tab bar",
    }
}

/// A Cupertino controls surface: every button style at rest, both switch
/// states, and the activity indicator at a fixed frame time.
fn cupertino_controls() -> CorpusCase {
    fn styled(style: frust_cupertino::CupertinoButtonStyle) -> AnyView<()> {
        any(container::<(), _>(
            frust_cupertino::cupertino_button::<(), _>("", |_: &mut ()| {}).style(style),
        )
        .size(180.0, 50.0))
    }
    fn record(scene: &mut Scene) {
        record_page(scene, frust_cupertino::baseline, |family| {
            use frust_cupertino::CupertinoButtonStyle::{Filled, Glass, Gray};
            body(
                IOS_GROUPED_BG,
                vec![
                    any(dark_on_light(family, SAMPLE_TEXT, 20.0)),
                    gap(),
                    styled(Filled),
                    gap(),
                    styled(Gray),
                    gap(),
                    styled(Glass),
                    gap(),
                    any(Row(vec![
                        any(frust_cupertino::cupertino_switch::<(), _>(
                            true,
                            |_: &mut (), _: bool| {},
                        )),
                        hgap(),
                        any(frust_cupertino::cupertino_switch::<(), _>(
                            false,
                            |_: &mut (), _: bool| {},
                        )),
                    ])),
                    gap(),
                    any(frust_cupertino::cupertino_activity_indicator()),
                ],
            )
        });
    }
    CorpusCase {
        spec: case("page-cupertino-controls"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "Cupertino button styles, both switch states, and the activity indicator",
    }
}

// ---------------------------------------------------------------------------
// Glyph
// ---------------------------------------------------------------------------

/// A Glyph dashboard: cards with their title/description/footer VIEW slots
/// filled with pinned text, a determinate progress track, and the dots loader
/// at a fixed frame time.
fn glyph_dashboard() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_page(scene, frust_glyph::baseline, |family| {
            let card = |size: f32| {
                any(frust_glyph::glyph_card::<()>()
                    .title(light_on_dark(family, SAMPLE_TEXT, size))
                    .desc(light_on_dark(family, SAMPLE_TEXT_LONG, 13.0))
                    .footer(light_on_dark(family, SAMPLE_TEXT, 12.0)))
            };
            body(
                GLYPH_BG,
                vec![
                    any(light_on_dark(family, SAMPLE_TEXT, 24.0)),
                    gap(),
                    card(18.0),
                    gap(),
                    card(16.0),
                    gap(),
                    any(frust_glyph::progress(0.42)),
                    gap(),
                    any(frust_glyph::dots_loader()),
                ],
            )
        });
    }
    CorpusCase {
        spec: case("page-glyph-dashboard"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "a Glyph dashboard: card stack with filled slots, progress track and dots loader",
    }
}

/// A Glyph loading surface: skeleton placeholders at three widths, a progress
/// track at both ends, and a card whose description slot is itself a skeleton
/// — the states a catalog page shows before its data arrives.
fn glyph_surfaces() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_page(scene, frust_glyph::baseline, |family| {
            body(
                GLYPH_BG,
                vec![
                    any(light_on_dark(family, SAMPLE_TEXT, 22.0)),
                    gap(),
                    any(frust_glyph::skeleton(320.0, 24.0)),
                    gap(),
                    any(frust_glyph::skeleton(240.0, 16.0)),
                    gap(),
                    any(frust_glyph::skeleton(160.0, 16.0)),
                    gap(),
                    any(frust_glyph::progress(0.0)),
                    gap(),
                    any(frust_glyph::progress(1.0)),
                    gap(),
                    any(frust_glyph::glyph_card::<()>()
                        .title(light_on_dark(family, SAMPLE_TEXT, 16.0))
                        .desc(frust_glyph::skeleton(200.0, 14.0))),
                ],
            )
        });
    }
    CorpusCase {
        spec: case("page-glyph-surfaces"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "Glyph loading surfaces: skeleton placeholders and progress at both ends",
    }
}

// ---------------------------------------------------------------------------
// shadcn
// ---------------------------------------------------------------------------

/// A shadcn form card: the card's own slot structure (content, separator,
/// footer) filled with pinned text, a progress track and a two-variant button
/// row.
fn shadcn_form() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_page(scene, frust_shadcn::theme, |family| {
            let content = Column(vec![
                any(dark_on_light(family, SAMPLE_TEXT, 18.0)),
                any(dark_on_light(family, SAMPLE_TEXT_LONG, 13.0)),
                gap(),
                any(frust_shadcn::progress(0.55)),
            ]);
            let card = frust_shadcn::card::<()>(vec![
                frust_shadcn::card_content::<()>(content),
                any(frust_shadcn::separator()),
                frust_shadcn::card_footer::<()>(vec![
                    any(frust_shadcn::button::<()>("", |_: &mut ()| {})),
                    hgap(),
                    any(frust_shadcn::button::<()>("", |_: &mut ()| {})
                        .variant(frust_shadcn::ButtonVariant::Outline)),
                ]),
            ]);
            body(
                Color::WHITE,
                vec![
                    any(dark_on_light(family, SAMPLE_TEXT, 22.0)),
                    gap(),
                    any(card),
                    gap(),
                    any(frust_shadcn::skeleton(280.0, 18.0)),
                ],
            )
        });
    }
    CorpusCase {
        spec: case("page-shadcn-form"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "a shadcn card with content/separator/footer slots, a progress track and buttons",
    }
}

/// A shadcn controls surface: switch and checkbox in both states, a
/// separator, and the progress/skeleton placeholders.
fn shadcn_controls() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_page(scene, frust_shadcn::theme, |family| {
            body(
                Color::WHITE,
                vec![
                    any(dark_on_light(family, SAMPLE_TEXT, 20.0)),
                    gap(),
                    any(Row(vec![
                        any(frust_shadcn::switch::<(), _>(
                            true,
                            |_: &mut (), _: bool| {},
                        )),
                        hgap(),
                        any(frust_shadcn::switch::<(), _>(
                            false,
                            |_: &mut (), _: bool| {},
                        )),
                    ])),
                    gap(),
                    any(Row(vec![
                        any(frust_shadcn::checkbox::<(), _>(
                            true,
                            |_: &mut (), _: bool| {},
                        )),
                        hgap(),
                        any(frust_shadcn::checkbox::<(), _>(
                            false,
                            |_: &mut (), _: bool| {},
                        )),
                    ])),
                    gap(),
                    any(frust_shadcn::separator()),
                    gap(),
                    any(frust_shadcn::progress(0.25)),
                    gap(),
                    any(frust_shadcn::skeleton(300.0, 20.0)),
                    gap(),
                    any(dark_on_light(family, SAMPLE_TEXT_LONG, 14.0)),
                ],
            )
        });
    }
    CorpusCase {
        spec: case("page-shadcn-controls"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "shadcn switch and checkbox in both states, separator, progress and skeleton",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{foreign_font_runs, glyph_run_count, scene_fingerprint};
    use std::collections::BTreeSet;

    #[test]
    fn every_case_name_is_unique_and_page_prefixed() {
        let mut seen = BTreeSet::new();
        for case in page_cases() {
            assert!(
                case.spec.name.starts_with("page-"),
                "`{}` must carry the page- golden-file prefix",
                case.spec.name
            );
            assert!(
                seen.insert(case.spec.name),
                "duplicate case name `{}`",
                case.spec.name
            );
        }
    }

    #[test]
    fn the_corpus_is_the_size_the_card_asks_for() {
        let count = page_cases().len();
        assert!(
            (6..=8).contains(&count),
            "the page corpus is 6-8 pages, got {count}"
        );
    }

    #[test]
    fn all_four_catalogs_are_represented() {
        let names: Vec<&str> = page_cases().into_iter().map(|c| c.spec.name).collect();
        for catalog in ["material", "cupertino", "glyph", "shadcn"] {
            assert!(
                names.iter().any(|n| n.contains(catalog)),
                "no page reconstructs the `{catalog}` catalog: {names:?}"
            );
        }
    }

    #[test]
    fn every_page_records_commands_and_shapes_text() {
        for case in page_cases() {
            let scene = case.scene();
            assert!(
                !scene.commands().is_empty(),
                "`{}` recorded nothing",
                case.spec.name
            );
            assert!(
                glyph_run_count(&scene) > 0,
                "`{}` is a catalog page and shaped no text",
                case.spec.name
            );
        }
    }

    #[test]
    fn no_page_shapes_against_a_host_font() {
        for case in page_cases() {
            let foreign = foreign_font_runs(&case.scene());
            assert!(
                foreign.is_empty(),
                "`{}` is not portable:\n{}",
                case.spec.name,
                foreign.join("\n")
            );
        }
    }

    #[test]
    fn a_page_is_a_pure_function_of_its_inputs() {
        // Two independent captures of the same case must record identical
        // command lists — the deterministic-state contract in this module's
        // docs, checked rather than asserted in prose.
        for case in page_cases() {
            let first = scene_fingerprint(&case.scene());
            let second = scene_fingerprint(&case.scene());
            assert_eq!(first, second, "`{}` is not deterministic", case.spec.name);
        }
    }
}
