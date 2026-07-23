//! Content section — reference §06 Content + Data: `glyph_card`, the
//! stat-card grid, `glyph_list` (doubling as the data-table stand-in — the
//! reference build's table has no dedicated Frust widget), an accordion
//! trio, an `empty_state`, the `term_block` (staggered, with a replay
//! button), and a `tooltip`.
//!
//! Every demo below is purely local interactivity (which row was last
//! pressed, which accordion panel is open, the term block's replay epoch) —
//! nothing this page needs lives in [`CatalogState`], so rather than grow the
//! shared state (out of this task's scope — see `pages/mod.rs`'s page-fn
//! contract), the page hosts one nested [`frust::component`] owning its own
//! retained [`ContentDemoState`], per `docs/CODE_STANDARDS.md`'s "local state
//! lives in the retained `Component` element" rule.

use frust::glyph::{
    BadgeVariant, StatDelta, TermLine, accordion, badge, empty_state, glyph_card, glyph_list,
    glyph_list_item, stat_card, term_block, tooltip,
};
use frust::{
    AnyView, Axis, ButtonStyle, Color, Column, Component, FlexView, Row, SizedBox, any, button,
    component, flexible, inflexible, keyed, text,
};

use crate::CatalogState;

/// Section-heading accent color (mirrors the shell header's amber title in
/// `crate::header_row`).
const SECTION_ACCENT: Color = Color::from_rgb8(0xFF, 0xB6, 0x27);

/// Gap between a section heading and its body, in logical px.
const HEADING_GAP: f64 = 10.0;
/// Gap between one section and the next, in logical px.
const SECTION_GAP: f64 = 28.0;
/// Gap between adjacent stat cards in the 4-up grid, in logical px.
const STAT_GAP: f64 = 10.0;

/// See the page-fn contract in [`crate::pages`]. Content has no shared signal
/// to read — it only mounts [`ContentDemo`]'s own retained local state.
pub fn page(_state: &CatalogState) -> AnyView<CatalogState> {
    any(component(ContentDemo))
}

/// A titled section: an accent heading over `body`, with trailing space
/// before the next section.
fn section(title: &str, body: AnyView<ContentDemoState>) -> AnyView<ContentDemoState> {
    any(Column(vec![
        any(text(title).size(15.0).color(SECTION_ACCENT)),
        any(SizedBox(None, Some(HEADING_GAP))),
        body,
        any(SizedBox(None, Some(SECTION_GAP))),
    ]))
}

/// [`ContentDemo`]'s retained local state (see the [module docs](self)): the
/// exclusively-open accordion panel, the last-pressed list row (echoed in a
/// caption), and a replay epoch that re-keys the term block to restart its
/// staggered reveal.
struct ContentDemoState {
    /// The single open accordion panel (0..3); pressing a header always
    /// opens *that* panel — one is open, never zero (the reference's "3
    /// items, 1 open" trio).
    open_accordion: usize,
    /// The index of the [`glyph_list`] row last pressed, if any.
    pressed_row: Option<usize>,
    /// Incremented by the Replay button; re-keying the term block on this
    /// value forces a fresh widget (and thus a fresh stagger controller) —
    /// the "re-key/rebuild to restart the stagger" semantics API.md notes.
    replay_epoch: u64,
}

/// The content page's own [`Component`] (see the [module docs](self)).
struct ContentDemo;

impl Component for ContentDemo {
    type State = ContentDemoState;

    fn init(&self) -> ContentDemoState {
        ContentDemoState {
            open_accordion: 1,
            pressed_row: None,
            replay_epoch: 0,
        }
    }

    fn build(&self, state: &mut ContentDemoState) -> AnyView<ContentDemoState> {
        any(Column(vec![
            section("Card", card_demo()),
            section("Stat cards", stat_card_grid()),
            section("List (also the data-table stand-in)", list_demo(state)),
            section("Accordion", accordion_demo(state)),
            section("Empty state", empty_state_demo()),
            section("Terminal", term_demo(state)),
            section("Tooltip", tooltip_demo()),
        ]))
    }
}

/// A `glyph_card` with all three slots: title, description, and a footer
/// pairing a status badge with a small secondary button.
fn card_demo() -> AnyView<ContentDemoState> {
    any(glyph_card()
        .title(text("Deploy pipeline"))
        .desc(text(
            "Builds, tests, and ships the release artifact on every push to main.",
        ))
        .footer(Row(vec![
            any(badge("stable", BadgeVariant::Success).dot(true)),
            any(SizedBox(Some(8.0), None)),
            any(button("Details", |_: &mut ContentDemoState| {})
                .style(ButtonStyle::Secondary)
                .small()),
        ])))
}

/// A 4-up `stat_card` grid mixing an [`StatDelta::Up`] and a
/// [`StatDelta::Down`] delta alongside two plain readouts.
fn stat_card_grid() -> AnyView<ContentDemoState> {
    any(FlexView::new(
        Axis::Horizontal,
        vec![
            flexible(1, stat_card("Sessions", "1,284")),
            inflexible(SizedBox(Some(STAT_GAP), None)),
            flexible(
                1,
                stat_card("Uptime", "99.98%").delta(StatDelta::Up, "0.4%"),
            ),
            inflexible(SizedBox(Some(STAT_GAP), None)),
            flexible(1, stat_card("Errors", "12").delta(StatDelta::Down, "3")),
            inflexible(SizedBox(Some(STAT_GAP), None)),
            flexible(1, stat_card("Latency", "42ms")),
        ],
    ))
}

/// A `glyph_list` of 4 rows (glyph box, title, sub, meta, chevron) that also
/// stands in for the reference build's data table, plus a caption echoing the
/// last-pressed row index.
fn list_demo(state: &ContentDemoState) -> AnyView<ContentDemoState> {
    let press_note = match state.pressed_row {
        Some(i) => format!("Row {i} pressed."),
        None => "Tap a row to see it announced here.".to_string(),
    };
    let caption = format!(
        "This list doubles as the data-table stand-in — the reference build's \
         table has no dedicated Frust widget. {press_note}"
    );

    any(Column(vec![
        any(glyph_list(vec![
            glyph_list_item("$", "deploy")
                .sub("main → production")
                .meta("2m ago")
                .chevron(true),
            glyph_list_item("#", "rollback")
                .sub("v0.44.0 → v0.43.2")
                .meta("1h ago")
                .chevron(true),
            glyph_list_item("~", "migrate")
                .sub("schema v12")
                .meta("3h ago")
                .chevron(true),
            glyph_list_item("!", "alert")
                .sub("latency spike")
                .meta("6h ago")
                .chevron(true),
        ])
        .on_press(|state: &mut ContentDemoState, i| state.pressed_row = Some(i))),
        any(SizedBox(None, Some(6.0))),
        any(text(caption).size(11.0)),
    ]))
}

/// An accordion trio, one panel open at a time — the built-in height
/// animation plays on every toggle.
fn accordion_demo(state: &ContentDemoState) -> AnyView<ContentDemoState> {
    let open = state.open_accordion;
    any(Column(vec![
        accordion_item(
            0,
            open,
            "What is Glyph?",
            "A terminal-native design language: monospace type, amber accent, minimal chrome.",
        ),
        any(SizedBox(None, Some(8.0))),
        accordion_item(
            1,
            open,
            "Reduced motion",
            "Every transition collapses to a 120ms linear crossfade when the OS \
             accessibility flag is on.",
        ),
        any(SizedBox(None, Some(8.0))),
        accordion_item(
            2,
            open,
            "Data-table stand-in",
            "No dedicated table widget ships yet — glyph_list plays that role \
             (see the List section above).",
        ),
    ]))
}

/// One accordion panel at `index`, open iff `index == open`. Pressing its
/// header always opens *that* panel (an exclusive group never collapses to
/// zero open).
fn accordion_item(index: usize, open: usize, title: &str, body: &str) -> AnyView<ContentDemoState> {
    any(accordion(title, text(body)).open(open == index).on_toggle(
        move |state: &mut ContentDemoState| {
            state.open_accordion = index;
        },
    ))
}

/// An `empty_state` panel with a glyph, title/description, and a CTA button.
fn empty_state_demo() -> AnyView<ContentDemoState> {
    any(empty_state(
        "No deployments yet",
        "Trigger your first deploy to see activity here.",
    )
    .glyph("▪")
    .action(button("Deploy now", |_: &mut ContentDemoState| {}).style(ButtonStyle::Primary)))
}

/// A staggered `term_block` plus a Replay button. Per API.md's semantics, a
/// staggered reveal restarts only on a fresh widget build — so the block is
/// wrapped in a single [`keyed`] flex child, re-keyed by
/// [`ContentDemoState::replay_epoch`] on every press (a full teardown +
/// rebuild, never a content-diff `rebuild`).
fn term_demo(state: &ContentDemoState) -> AnyView<ContentDemoState> {
    let lines = vec![
        TermLine::prompt("frust build apk --release"),
        TermLine::output("Compiling glyph-catalog v0.1.0"),
        TermLine::output("Finished release [optimized] target(s) in 38.2s"),
        TermLine::comment("# staggered per-line reveal — replays below"),
    ];

    any(Column(vec![
        any(FlexView::new(
            Axis::Vertical,
            vec![keyed(state.replay_epoch, term_block(lines).staggered(true))],
        )),
        any(SizedBox(None, Some(8.0))),
        any(button("Replay", |state: &mut ContentDemoState| {
            state.replay_epoch += 1;
        })
        .small()),
    ]))
}

/// A `tooltip` wrapping a small button target, plus a caption on its
/// brightness-invariant ink (unlike every other themed color on this page,
/// `GlyphInk` never swaps between light and dark).
fn tooltip_demo() -> AnyView<ContentDemoState> {
    any(Column(vec![
        any(tooltip(
            button("Hold me", |_: &mut ContentDemoState| {})
                .style(ButtonStyle::Secondary)
                .small(),
            "Long-press to peek",
        )),
        any(SizedBox(None, Some(6.0))),
        any(text(
            "Tooltip ink stays fixed (GlyphInk) — it never swaps with the page's \
             light/dark brightness, unlike every other color on this page.",
        )
        .size(11.0)),
    ]))
}
