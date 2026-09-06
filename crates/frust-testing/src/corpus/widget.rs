//! The WIDGET corpus: the baseline `frust-widgets` set rendered through a
//! real [`frust_core::RenderRoot`], one named case per interaction state that
//! `docs/TESTING.md`'s Coverage Catalog asks for.
//!
//! # What this corpus is for
//!
//! [`super::unit`] pins one [`frust_scene::Command`] at a time, recorded by
//! hand. These cases pin what the WIDGET LAYER emits: a button's fill, border,
//! radius, state layer and disabled alpha; a field's chrome, focus ring and
//! caret; a virtualized list's materialized window and its clip; a flex row's
//! distribution. Nothing here draws a design system — that is
//! [`super::page`]'s job; everything here is `frust-widgets` plus
//! [`frust_theme::Theme::neutral`].
//!
//! # Frame
//!
//! Every case is one frame of a 412x892 LOGICAL phone at scale 2.0
//! ([`FrameSpec::phone`]), captured through [`crate::frame::frame`] — layout
//! logical, device scale pushed at paint, exactly as a shell does it. The
//! stored baseline is therefore 824x1784 PHYSICAL pixels and its
//! [`CaseSpec::scale`](crate::case::CaseSpec::scale) is 1.0 (see
//! [`crate::frame`]'s module docs).
//!
//! # State
//!
//! An interaction state is reached the way a user reaches it — a synthetic
//! pointer event dispatched through [`frust_core::RenderRoot::event`] — never
//! by constructing a widget's private state. `widget-button-pressed` holds a
//! pointer DOWN with no matching up; `widget-text-field-caret` taps the field
//! to focus it and paints at [`frust_core::FrameTime::ZERO`], where the blink phase is
//! zero and the caret is lit. A case that dispatches an event captures a
//! warm-up frame first, because a pointer event can only hit geometry that a
//! previous layout pass produced.
//!
//! # Text, and the labels that are deliberately empty
//!
//! Every glyph in a committed baseline must come from a BUNDLED face, or the
//! baseline is runner-local — [`crate::frame::foreign_font_runs`] is the gate,
//! and `tests/page_goldens.rs` enforces it on every case here.
//!
//! Text this corpus authors itself is pinned with
//! [`crate::frame::pinned_text_style`]. Text a widget authors INTERNALLY is
//! pinnable only when the widget resolves its style from the theme's type
//! scale. In today's baseline set it does not: `frust_widgets::button`,
//! `checkbox` and `radio` build their label as
//! `text(label)` — i.e. `TextStyle::default()`, whose family is
//! [`frust_text::FontFamily::SystemUi`] — and take only the label's COLOR from
//! the theme. There is no seam to override that from a caller, so those cases
//! pass an EMPTY label and pin the widget's chrome (fill, border, radius,
//! state layer, disabled alpha, check/knob geometry) rather than its
//! typography. Labelled-widget typography is covered by the cases built on
//! [`frust_widgets::text`] and [`frust_widgets::text_input`], both of which do
//! take a caller-supplied style. Closing that gap is a `frust-widgets` change
//! (a label style/child seam), not something a corpus can work around.

use frust_core::{AnyView, RenderRoot, View, any};
use frust_scene::Scene;
use frust_text::{FontFamily, TextOverflow, TextStyle};
use frust_theme::Theme;
use kurbo::Point;
use peniko::Color;

use super::CorpusCase;
use crate::case::{CaseSpec, Tolerance};
use crate::frame::{
    FrameSpec, SAMPLE_TEXT, SAMPLE_TEXT_LONG, frame, physical_case, pin_type_scale,
    pinned_text_style, press_at, tap_at, test_text_context,
};

/// Every widget case's `()`-state root view type.
type Root = RenderRoot<(), AnyView<()>>;

/// Ink for corpus-authored text: near-black, so a glyph's coverage reads
/// against the white base without being a pure-black cliff.
const INK: Color = Color::from_rgb8(0x14, 0x14, 0x18);

/// A tinted panel fill, distinct from both the white base and the theme's own
/// surfaces, so a container's geometry is legible in a diff.
const PANEL: Color = Color::from_rgb8(0xE3, 0xE8, 0xF0);

/// A second panel fill for the alternating rows of the list case.
const PANEL_ALT: Color = Color::from_rgb8(0xF3, 0xF5, 0xF9);

/// Hairline colour for the divider/list separators.
const HAIRLINE: Color = Color::from_rgb8(0xC2, 0xC8, 0xD2);

/// Outer padding every case lays its content inside, in LOGICAL px — one
/// value, so a case's geometry is comparable to its siblings' at a glance.
const GUTTER: f64 = 24.0;

/// The fixed width every pressable in the button cases is stretched to, in
/// LOGICAL px. Fixed rather than natural because a pressed case dispatches a
/// pointer event at a computed centre, and a natural width would move that
/// centre whenever a label or a padding token changed.
const CONTROL_W: f64 = 240.0;

/// The fixed height of the same pressables, in LOGICAL px.
const CONTROL_H: f64 = 52.0;

/// Vertical gap between stacked controls, in LOGICAL px.
const GAP: f64 = 16.0;

/// The centre of the Nth stacked control in the button cases, in LOGICAL
/// coordinates — where a synthetic press must land to hit it.
fn control_centre(index: usize) -> Point {
    let top = GUTTER + (CONTROL_H + GAP) * index as f64;
    Point::new(GUTTER + CONTROL_W / 2.0, top + CONTROL_H / 2.0)
}

/// The whole widget corpus, in declaration order.
#[must_use]
pub fn widget_cases() -> Vec<CorpusCase> {
    vec![
        button_rest(),
        button_pressed(),
        button_disabled(),
        text_field_rest(),
        text_field_caret(),
        list_20_rows(),
        checkbox_radio(),
        slider_positions(),
        scaffold_chrome(),
        flex_layout(),
        stack_align(),
        scroll_clipped(),
        text_wrap(),
    ]
}

/// The case shape every widget case takes: a phone frame at the crate's tight
/// default tolerance.
///
/// The tolerance is the default (channel 2, alpha 2, zero tolerated
/// mismatched pixels), NOT [`Tolerance::exact`]: these frames are full of
/// antialiased curves and shaped glyphs, where a one-step rounding difference
/// is expected and a whole-pixel one is not. Nothing widens it per run.
fn case(name: &'static str) -> CaseSpec {
    physical_case(name, &FrameSpec::phone(), Tolerance::new())
}

/// The theme every widget case installs: [`Theme::neutral`] with its whole
/// type scale pinned to the bundled face (see the module docs).
fn corpus_theme(family: &FontFamily) -> Theme {
    pin_type_scale(Theme::neutral(), family)
}

/// Corpus-authored body text at `size`, pinned to the bundled face.
fn label(family: &FontFamily, content: &str, size: f32) -> frust_widgets::TextView {
    frust_widgets::text(content).style(pinned_text_style(family, size, INK))
}

/// A fixed-size slot holding `child` — how a control is given the exact
/// geometry [`control_centre`] predicts.
fn slot<V: View<()> + 'static>(child: V) -> AnyView<()> {
    any(frust_widgets::SizedBox::<()>(Some(CONTROL_W), Some(CONTROL_H)).child(child))
}

/// A vertical gap of [`GAP`] logical px.
fn gap() -> AnyView<()> {
    any(frust_widgets::SizedBox::<()>(None, Some(GAP)))
}

/// Wraps `children` in the corpus's standard gutter-padded, white-filled
/// page body.
fn body(children: Vec<AnyView<()>>) -> AnyView<()> {
    any(frust_widgets::container::<(), _>(frust_widgets::Padding(
        frust_widgets::EdgeInsets::all(GUTTER),
        frust_widgets::Column(children),
    ))
    .fill(Color::WHITE)
    .expand())
}

/// Renders `build` into `scene` after `prepare` has driven whatever events the
/// case's state needs.
///
/// The two-frame shape is load-bearing for every interactive case: a pointer
/// event can only hit geometry a previous layout pass produced, so the warm-up
/// frame is laid out and painted (into a throwaway scene), the events are
/// dispatched, and only the SECOND frame is the one the baseline records.
fn record_with_events(
    scene: &mut Scene,
    build: impl Fn(&FontFamily) -> AnyView<()> + 'static,
    prepare: impl FnOnce(&mut Root, &mut ()),
) {
    let (mut tcx, family) = test_text_context();
    let spec = FrameSpec::phone();
    let mut root: Root = RenderRoot::new();
    root.set_theme(Box::new(corpus_theme(&family)));
    let mut state = ();
    let mut logic = move |_: &mut ()| build(&family);

    let mut warmup = Scene::new();
    frame(
        &mut root,
        &mut logic,
        &mut state,
        &mut tcx,
        &spec,
        &mut warmup,
    );
    prepare(&mut root, &mut state);
    frame(&mut root, &mut logic, &mut state, &mut tcx, &spec, scene);
}

/// Renders `build` into `scene` with no events dispatched — the resting shape
/// most cases take.
fn record_still(scene: &mut Scene, build: impl Fn(&FontFamily) -> AnyView<()> + 'static) {
    record_with_events(scene, build, |_, _| {});
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

/// Every [`frust_widgets::ButtonStyle`], stacked at a fixed size, in the
/// enabled/unpressed state.
///
/// Labels are empty deliberately — see the module docs' text section. What
/// this pins is each style's fill, border and corner radius against the
/// neutral theme, which is exactly the part a theme-token or shape-scale
/// regression moves.
fn button_stack(disabled: bool) -> AnyView<()> {
    use frust_widgets::ButtonStyle::{Danger, Ghost, Icon, Primary, Secondary};
    let mut children = Vec::new();
    for (index, style) in [Primary, Secondary, Ghost, Danger, Icon]
        .into_iter()
        .enumerate()
    {
        if index > 0 {
            children.push(gap());
        }
        children.push(slot(
            frust_widgets::button::<(), _>("", |_: &mut ()| {})
                .style(style)
                .disabled(disabled),
        ));
    }
    body(children)
}

fn button_rest() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_still(scene, |_| button_stack(false));
    }
    CorpusCase {
        spec: case("widget-button-rest"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "every baseline ButtonStyle at rest, enabled, on the neutral theme",
    }
}

/// The same stack with a pointer held DOWN on the primary button — the
/// pressed state layer, which a full tap would have already released.
fn button_pressed() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_with_events(
            scene,
            |_| button_stack(false),
            |root, state| press_at(root, state, control_centre(0)),
        );
    }
    CorpusCase {
        spec: case("widget-button-pressed"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "the primary button's pressed state layer, under a held pointer down",
    }
}

/// The same stack with every button disabled — the `DISABLED_ALPHA`
/// multiplier applied to fill, border and label.
fn button_disabled() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_still(scene, |_| button_stack(true));
    }
    CorpusCase {
        spec: case("widget-button-disabled"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "every baseline ButtonStyle disabled — fill/border dimmed by DISABLED_ALPHA",
    }
}

// ---------------------------------------------------------------------------
// Text field
// ---------------------------------------------------------------------------

/// Three fields: an empty one showing its placeholder, one carrying text, and
/// a disabled one. Every text style is pinned, placeholder included (a
/// placeholder inherits family/weight/tracking from the field's own style).
fn field_stack(family: &FontFamily) -> AnyView<()> {
    let style = pinned_text_style(family, 16.0, INK);
    let field = |value: &str, placeholder: &str, enabled: bool| {
        slot(
            frust_widgets::text_input::<(), _>(value.to_string(), |_: &mut (), _: String| {})
                .placeholder(placeholder.to_string())
                .text_style(style.clone())
                .enabled(enabled),
        )
    };
    body(vec![
        field("", SAMPLE_TEXT, true),
        gap(),
        field(SAMPLE_TEXT_LONG, "", true),
        gap(),
        field(SAMPLE_TEXT, "", false),
    ])
}

fn text_field_rest() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_still(scene, field_stack);
    }
    CorpusCase {
        spec: case("widget-text-field-rest"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "text fields unfocused: placeholder, value, and the disabled wash",
    }
}

/// The middle field focused by a real tap, painted at [`frust_core::FrameTime::ZERO`]
/// where the blink phase is zero and the caret is LIT — the state
/// `docs/TESTING.md`'s Inputs row calls "focused", plus the caret itself.
fn text_field_caret() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_with_events(scene, field_stack, |root, state| {
            tap_at(root, state, control_centre(1));
        });
    }
    CorpusCase {
        spec: case("widget-text-field-caret"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "a focused text field at blink phase zero: focus ring plus a lit caret",
    }
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

/// A virtualized [`frust_widgets::list_view`] of 20 uniform rows.
///
/// The frame is shorter than the content, so what the baseline records is the
/// MATERIALIZED WINDOW and its clip — a virtualization regression (a row too
/// many, a row too few, an off-by-one window origin) changes these pixels.
fn list_20_rows() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_still(scene, |family| {
            let family = family.clone();
            any(
                frust_widgets::container::<(), _>(frust_widgets::list_view::<()>(
                    20,
                    72.0,
                    move |index| {
                        let fill = if index % 2 == 0 { PANEL } else { PANEL_ALT };
                        any(frust_widgets::container::<(), _>(frust_widgets::Padding(
                            frust_widgets::EdgeInsets::symmetric(GUTTER, 20.0),
                            frust_widgets::text(SAMPLE_TEXT)
                                .style(pinned_text_style(&family, 18.0, INK)),
                        ))
                        .fill(fill))
                    },
                ))
                .fill(Color::WHITE)
                .expand(),
            )
        });
    }
    CorpusCase {
        spec: case("widget-list-20-rows"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "a 20-row virtualized list at offset zero: the materialized window and its clip",
    }
}

// ---------------------------------------------------------------------------
// Selection controls
// ---------------------------------------------------------------------------

/// Checkbox and radio in both of their states, plus a divider between the
/// pairs. Labels are empty (module docs); the box/tick and ring/dot geometry
/// is what this pins.
fn checkbox_radio() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_still(scene, |family| {
            body(vec![
                any(label(family, SAMPLE_TEXT, 20.0)),
                gap(),
                any(frust_widgets::checkbox::<(), _>(
                    false,
                    "",
                    |_: &mut (), _: bool| {},
                )),
                gap(),
                any(frust_widgets::checkbox::<(), _>(
                    true,
                    "",
                    |_: &mut (), _: bool| {},
                )),
                gap(),
                any(frust_widgets::divider(HAIRLINE).thickness(2.0)),
                gap(),
                any(frust_widgets::radio::<()>(false, "")),
                gap(),
                any(frust_widgets::radio::<()>(true, "")),
            ])
        });
    }
    CorpusCase {
        spec: case("widget-checkbox-radio"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "checkbox and radio in both states, separated by a themed divider",
    }
}

/// Sliders at the two ends and two interior fractions — the track/fill split
/// and the thumb's clamped travel.
fn slider_positions() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_still(scene, |family| {
            let mut children = vec![any(label(family, SAMPLE_TEXT, 20.0)), gap()];
            for value in [0.0_f64, 0.35, 0.5, 1.0] {
                children.push(slot(frust_widgets::slider::<(), _>(
                    value,
                    |_: &mut (), _: f64| {},
                )));
                children.push(gap());
            }
            body(children)
        });
    }
    CorpusCase {
        spec: case("widget-slider"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "sliders at 0.0/0.35/0.5/1.0 — track, active fill, and thumb travel",
    }
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// A [`frust_widgets::scaffold`] with all four slots filled: app bar, body,
/// bottom bar and a floating action slot. Pins the scaffold's own geometry —
/// bar heights, body inset, FAB margin and alignment.
fn scaffold_chrome() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_still(scene, |family| {
            let bar = |fill: Color, height: f64, family: &FontFamily| {
                any(frust_widgets::container::<(), _>(frust_widgets::Padding(
                    frust_widgets::EdgeInsets::symmetric(GUTTER, 12.0),
                    label(family, SAMPLE_TEXT, 18.0),
                ))
                .fill(fill)
                .size(f64::INFINITY, height))
            };
            any(frust_widgets::scaffold::<(), _>(frust_widgets::Padding(
                frust_widgets::EdgeInsets::all(GUTTER),
                frust_widgets::Column(vec![
                    any(label(family, SAMPLE_TEXT_LONG, 16.0)),
                    gap(),
                    any(frust_widgets::colored_box::<()>()
                        .fill(PANEL)
                        .size(200.0, 120.0)
                        .radius(16.0)),
                ]),
            ))
            .app_bar(bar(PANEL, 64.0, family))
            .bottom_bar(bar(PANEL_ALT, 72.0, family))
            .fab(any(frust_widgets::colored_box::<()>()
                .fill(HAIRLINE)
                .size(56.0, 56.0)
                .radius(28.0)))
            .background(Color::WHITE))
        });
    }
    CorpusCase {
        spec: case("widget-scaffold-chrome"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "a scaffold with app bar, body, bottom bar and FAB — slot geometry",
    }
}

/// Flex distribution: the flexible/inflexible share split and every
/// [`frust_widgets::CrossAxisAlignment`], drawn with fixed-size
/// [`frust_widgets::test_support`] leaves and plain filled boxes so nothing
/// but the CONTAINER's arithmetic is under test.
///
/// Main-axis alignment is not varied: the baseline flex ships only
/// [`frust_widgets::MainAxisAlignment::Start`] today, so the share split
/// (`flex: 0` / `1` / `2` over the same free space) is what carries the
/// distribution coverage.
fn flex_layout() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_still(scene, |_| {
            let tile = |w: f64, h: f64, fill: Color| {
                frust_widgets::colored_box::<()>().fill(fill).size(w, h)
            };
            let stretchy = |fill: Color| frust_widgets::colored_box::<()>().fill(fill);
            let shares = |a: u32, b: u32| {
                any(
                    frust_widgets::container::<(), _>(frust_widgets::FlexView::new(
                        frust_widgets::Axis::Horizontal,
                        vec![
                            frust_widgets::inflexible(tile(56.0, 40.0, PANEL)),
                            frust_widgets::flexible(a, stretchy(HAIRLINE)),
                            frust_widgets::flexible(b, stretchy(PANEL_ALT)),
                        ],
                    ))
                    .size(320.0, 40.0),
                )
            };
            let cross = |alignment: frust_widgets::CrossAxisAlignment| {
                any(frust_widgets::container::<(), _>(
                    frust_widgets::Row(vec![
                        any(frust_widgets::test_support::leaf(40.0, 24.0)),
                        any(tile(40.0, 48.0, PANEL)),
                        any(tile(40.0, 72.0, PANEL_ALT)),
                    ])
                    .cross_axis(alignment),
                )
                .size(320.0, 88.0)
                .border(HAIRLINE, 1.0))
            };
            use frust_widgets::CrossAxisAlignment::{Center, Start, Stretch};
            body(vec![
                shares(1, 1),
                gap(),
                shares(1, 2),
                gap(),
                shares(3, 1),
                gap(),
                cross(Start),
                gap(),
                cross(Center),
                gap(),
                cross(Stretch),
            ])
        });
    }
    CorpusCase {
        spec: case("widget-flex-layout"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "flex share splits (1:1, 1:2, 3:1) plus every CrossAxisAlignment",
    }
}

/// A [`frust_widgets::Stack`] of aligned children over a filled base, plus a
/// bordered/rounded container — z-order and [`frust_widgets::Alignment`]
/// arithmetic in one frame.
fn stack_align() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_still(scene, |family| {
            let pip = |x: f64, y: f64, fill: Color| {
                any(frust_widgets::Align::<(), _>(
                    frust_widgets::Alignment::new(x, y),
                    frust_widgets::colored_box::<()>()
                        .fill(fill)
                        .size(64.0, 64.0)
                        .radius(12.0),
                ))
            };
            body(vec![
                any(label(family, SAMPLE_TEXT, 20.0)),
                gap(),
                any(frust_widgets::container::<(), _>(frust_widgets::Stack(vec![
                    any(frust_widgets::colored_box::<()>().fill(PANEL_ALT).expand()),
                    pip(-1.0, -1.0, PANEL),
                    pip(0.0, 0.0, HAIRLINE),
                    pip(1.0, 1.0, INK),
                ]))
                .size(320.0, 320.0)
                .radius(24.0)
                .border(HAIRLINE, 3.0)),
            ])
        });
    }
    CorpusCase {
        spec: case("widget-stack-align"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "stacked, aligned children inside a bordered rounded container",
    }
}

/// A [`frust_widgets::scroll_view`] whose content is taller than its box, at
/// offset zero — the clip boundary is the contract.
fn scroll_clipped() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_still(scene, |family| {
            let family = family.clone();
            let rows: Vec<AnyView<()>> = (0..12)
                .map(|index| {
                    let fill = if index % 2 == 0 { PANEL } else { PANEL_ALT };
                    any(frust_widgets::container::<(), _>(frust_widgets::Padding(
                        frust_widgets::EdgeInsets::all(12.0),
                        frust_widgets::text(SAMPLE_TEXT)
                            .style(pinned_text_style(&family, 18.0, INK)),
                    ))
                    .fill(fill)
                    .size(f64::INFINITY, 64.0))
                })
                .collect();
            body(vec![any(frust_widgets::container::<(), _>(
                frust_widgets::scroll_view::<(), _>(frust_widgets::Column(rows)),
            )
            .size(f64::INFINITY, 420.0)
            .radius(20.0)
            .border(HAIRLINE, 2.0))])
        });
    }
    CorpusCase {
        spec: case("widget-scroll-clipped"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "a scroll view at offset zero, its overflowing content clipped to the box",
    }
}

/// Text at several sizes, wrapped against a narrow column, with a
/// max-lines/ellipsis arm — the one case whose whole subject is shaped
/// glyphs, all of them from the bundled face.
fn text_wrap() -> CorpusCase {
    fn record(scene: &mut Scene) {
        record_still(scene, |family| {
            // Deliberately short: this frame is 824x1784 physical pixels and
            // `tests/corpus_budget.rs` caps a single golden PNG at 64 KB, which
            // dense antialiased text reaches quickly. Two repetitions still wrap
            // to several lines in the 220px box below, which is the property
            // under test.
            let long = [SAMPLE_TEXT_LONG; 2].join(", ");
            let wrapped: TextStyle = pinned_text_style(family, 18.0, INK);
            body(vec![
                any(label(family, SAMPLE_TEXT, 32.0)),
                gap(),
                any(label(family, SAMPLE_TEXT, 20.0)),
                gap(),
                any(label(family, SAMPLE_TEXT, 12.0)),
                gap(),
                any(frust_widgets::container::<(), _>(
                    frust_widgets::text(long.clone()).style(wrapped.clone()),
                )
                .size(220.0, 240.0)),
                gap(),
                any(frust_widgets::container::<(), _>(
                    frust_widgets::text(long)
                        .style(wrapped)
                        .max_lines(2)
                        .overflow(TextOverflow::Ellipsis),
                )
                .size(220.0, 80.0)),
            ])
        });
    }
    CorpusCase {
        spec: case("widget-text-wrap"),
        record,
        probes: &[],
        eroded_interior: false,
        about: "shaped text at three sizes, wrapped in a narrow box, plus a 2-line ellipsis",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{foreign_font_runs, glyph_run_count, scene_fingerprint};
    use std::collections::BTreeSet;

    #[test]
    fn every_case_name_is_unique_and_widget_prefixed() {
        let mut seen = BTreeSet::new();
        for case in widget_cases() {
            assert!(
                case.spec.name.starts_with("widget-"),
                "`{}` must carry the widget- golden-file prefix",
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
        let count = widget_cases().len();
        assert!(
            (10..=15).contains(&count),
            "the widget corpus is 10-15 cases, got {count}"
        );
    }

    #[test]
    fn every_case_records_a_non_empty_scene_at_the_phone_size() {
        for case in widget_cases() {
            let scene = case.scene();
            assert!(
                !scene.commands().is_empty(),
                "`{}` recorded nothing",
                case.spec.name
            );
            assert_eq!(case.spec.width, 824, "{}", case.spec.name);
            assert_eq!(case.spec.height, 1784, "{}", case.spec.name);
            assert_eq!(
                case.spec.scale, 1.0,
                "`{}` must not scale twice",
                case.spec.name
            );
        }
    }

    #[test]
    fn no_case_shapes_against_a_host_font() {
        for case in widget_cases() {
            let scene = case.scene();
            let foreign = foreign_font_runs(&scene);
            assert!(
                foreign.is_empty(),
                "`{}` is not portable:\n{}",
                case.spec.name,
                foreign.join("\n")
            );
        }
    }

    #[test]
    fn the_text_cases_actually_shape_text() {
        for name in [
            "widget-text-wrap",
            "widget-list-20-rows",
            "widget-text-field-caret",
        ] {
            let case = widget_cases()
                .into_iter()
                .find(|c| c.spec.name == name)
                .expect("declared case");
            assert!(
                glyph_run_count(&case.scene()) > 0,
                "`{name}` exists to shape text and shaped none"
            );
        }
    }

    #[test]
    fn a_held_press_changes_the_frame() {
        let rest = widget_cases()
            .into_iter()
            .find(|c| c.spec.name == "widget-button-rest")
            .expect("declared case")
            .scene();
        let pressed = widget_cases()
            .into_iter()
            .find(|c| c.spec.name == "widget-button-pressed")
            .expect("declared case")
            .scene();
        assert!(!rest.commands().is_empty(), "both cases record commands");
        assert!(
            scene_fingerprint(&rest) != scene_fingerprint(&pressed),
            "the held pointer down must reach the button — the pressed frame is identical \
             to the resting one, so `control_centre(0)` missed its target"
        );
    }

    #[test]
    fn disabling_changes_the_frame() {
        let rest = widget_cases()
            .into_iter()
            .find(|c| c.spec.name == "widget-button-rest")
            .expect("declared case")
            .scene();
        let disabled = widget_cases()
            .into_iter()
            .find(|c| c.spec.name == "widget-button-disabled")
            .expect("declared case")
            .scene();
        assert!(
            scene_fingerprint(&rest) != scene_fingerprint(&disabled),
            "the disabled stack must differ from the resting one"
        );
    }
}
