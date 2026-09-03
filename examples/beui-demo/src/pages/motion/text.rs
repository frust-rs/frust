//! Motion · Text: the catalog's text, number and status effects —
//! [`text_animation`]'s five variants, [`number`]'s two modes,
//! [`animated_badge`], all seventeen [`loader`] variants, [`marquee`]'s four
//! directions, and [`theme_toggle`]'s four reveals.
//!
//! # Where the page's state lives
//!
//! `crate::main`'s `page_body` calls every Motion page as a bare
//! `page() -> AnyView<AppState>`, and `AppState` itself is outside this task's
//! write scope — so a page that needs retained state of its own hosts it in a
//! [`frust::component`] instead of a field on `AppState`. That is the same
//! `component(..)` route `examples/material3-demo`'s playgrounds take, and it
//! keeps every knob on this page genuinely interactive: the buttons write into
//! [`State`], the component rebuilds, and the components below re-read it.
//! Only the page's own heading and lead-in are built in `AppState` space, so
//! they can reuse `crate::nav`'s shared [`heading`]/[`caption`] styling.
//!
//! Sample data mirrors upstream's own previews
//! (`components/previews/motion/{text-*,number*,animated-badge,loader,marquee,
//! theme-toggle}.preview.tsx`) so the page reads like beui.dev: the same
//! phrases, the same 48,273 active-user counter, the same badge state machine,
//! the same logo ticker.

use frust::{
    AnyView, Column, Component, CrossAxisAlignment, MainAxisAlignment, Row, SizedBox, any,
    component, text,
};
use frust_beui::components::animated_badge::{
    AnimatedBadgeSize, AnimatedBadgeStatus, animated_badge,
};
use frust_beui::components::button::{ButtonSize, ButtonTone, button};
use frust_beui::components::loader::{LoaderVariant, loader};
use frust_beui::components::marquee::{MarqueeDirection, marquee};
use frust_beui::components::number::{NumberMode, number};
use frust_beui::components::text_animation::{TextAnimationVariant, text_animation};
use frust_beui::components::theme_toggle::{ThemeToggleStart, ThemeToggleVariant, theme_toggle};

use crate::AppState;
use crate::nav::{caption, heading};

/// The phrases the per-letter and scramble effects cycle through — upstream's
/// own `text-scramble` / `text-cascade` preview copy.
const PHRASES: [&str; 3] = [
    "Inspecting the repository",
    "Running the checks",
    "Preparing the update",
];

/// The badge's state machine, verbatim from `animated-badge.preview.tsx`
/// (upstream advances it on a 1.6s timer; here the "Advance" button is the
/// clock, so the roll is watchable rather than automatic).
const BADGE_STATES: [(AnimatedBadgeStatus, &str); 4] = [
    (AnimatedBadgeStatus::Loading, "Syncing"),
    (AnimatedBadgeStatus::Success, "Synced"),
    (AnimatedBadgeStatus::Warning, "Review"),
    (AnimatedBadgeStatus::Danger, "Failed"),
];

/// Every badge status, for the exhaustive tone row.
const BADGE_STATUSES: [(AnimatedBadgeStatus, &str); 6] = [
    (AnimatedBadgeStatus::Neutral, "Neutral"),
    (AnimatedBadgeStatus::Info, "Info"),
    (AnimatedBadgeStatus::Success, "Success"),
    (AnimatedBadgeStatus::Warning, "Warning"),
    (AnimatedBadgeStatus::Danger, "Danger"),
    (AnimatedBadgeStatus::Loading, "Loading"),
];

/// The marquee's ticker content — upstream's own logo list.
const LOGOS: [&str; 8] = [
    "Vercel", "Linear", "Stripe", "Figma", "GitHub", "Notion", "Loom", "Raycast",
];

/// The four `theme_toggle` reveals, in upstream's preview order.
const TOGGLE_VARIANTS: [(ThemeToggleVariant, &str); 4] = [
    (ThemeToggleVariant::Rectangle, "Rectangle"),
    (ThemeToggleVariant::Circle, "Circle"),
    (ThemeToggleVariant::CircleBlur, "Circle blur"),
    (ThemeToggleVariant::Blinds, "Blinds"),
];

/// Every reveal origin, for the start-position cycler.
const TOGGLE_STARTS: [(ThemeToggleStart, &str); 6] = [
    (ThemeToggleStart::BottomUp, "bottom-up"),
    (ThemeToggleStart::TopLeft, "top-left"),
    (ThemeToggleStart::TopRight, "top-right"),
    (ThemeToggleStart::BottomLeft, "bottom-left"),
    (ThemeToggleStart::BottomRight, "bottom-right"),
    (ThemeToggleStart::Center, "center"),
];

/// This page's retained knobs.
pub struct State {
    /// Index into [`PHRASES`] — changing it restarts every text effect, which
    /// is how `text_animation` replays (a content change calls `restart`).
    phrase: usize,
    /// The live counter both `number` modes read.
    value: f64,
    /// Index into [`BADGE_STATES`].
    badge: usize,
    /// The square every loader is drawn at.
    loader_size: f64,
    /// Which way the marquee travels.
    marquee_direction: MarqueeDirection,
    /// Whether the marquee paints its edge fade (off by default in this port —
    /// see the caption).
    marquee_fade: bool,
    /// Index into [`TOGGLE_STARTS`].
    toggle_start: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            phrase: 0,
            // `number-ticker.preview.tsx`'s own starting value.
            value: 48_273.0,
            badge: 0,
            loader_size: 36.0,
            marquee_direction: MarqueeDirection::Left,
            marquee_fade: false,
            toggle_start: 0,
        }
    }
}

/// A vertical spacer.
fn gap(height: f64) -> AnyView<State> {
    any(SizedBox(None, Some(height)))
}

/// A horizontal spacer.
fn hgap(width: f64) -> AnyView<State> {
    any(SizedBox(Some(width), None))
}

/// One component's block: its name, a one-line note (which is where a ported
/// degradation is stated), and the live instances.
fn demo(title: &str, note: &str, body: Vec<AnyView<State>>) -> AnyView<State> {
    let mut children = vec![
        any(text(title.to_string()).size(16.0)),
        gap(4.0),
        any(caption(note.to_string())),
        gap(12.0),
    ];
    children.extend(body);
    children.push(gap(32.0));
    any(Column(children).cross_axis(CrossAxisAlignment::Start))
}

/// A labelled specimen: the live component over its caption.
fn specimen(label: &str, body: AnyView<State>) -> AnyView<State> {
    any(
        Column(vec![body, gap(6.0), any(caption(label.to_string()))])
            .cross_axis(CrossAxisAlignment::Center),
    )
}

/// Lay `items` out in rows of `per_row`, `gap`px apart — this workspace has no
/// flow/wrap primitive, so a long variant grid is chunked by hand (the same
/// route `examples/material3-demo` takes for upstream's `Wrap`).
fn grid(items: Vec<AnyView<State>>, per_row: usize, spacing: f64) -> AnyView<State> {
    let mut rows: Vec<AnyView<State>> = Vec::new();
    let mut row: Vec<AnyView<State>> = Vec::new();
    for item in items {
        if !row.is_empty() {
            row.push(hgap(spacing));
        }
        row.push(item);
        if row.len() >= per_row * 2 - 1 {
            rows.push(any(
                Row(std::mem::take(&mut row)).cross_axis(CrossAxisAlignment::Start)
            ));
        }
    }
    if !row.is_empty() {
        rows.push(any(Row(row).cross_axis(CrossAxisAlignment::Start)));
    }
    let mut spaced: Vec<AnyView<State>> = Vec::with_capacity(rows.len() * 2);
    for r in rows {
        if !spaced.is_empty() {
            spaced.push(gap(spacing));
        }
        spaced.push(r);
    }
    any(Column(spaced).cross_axis(CrossAxisAlignment::Start))
}

/// A small ghost button — the knob every demo on this page is driven by.
fn knob(label: &str, on_press: impl Fn(&mut State) + 'static) -> AnyView<State> {
    any(button(label.to_string(), on_press)
        .tone(ButtonTone::Outline)
        .size(ButtonSize::Sm))
}

/// The `text_animation` block: all five variants over one phrase, plus the
/// phrase cycler that restarts them.
fn text_effects(state: &State) -> AnyView<State> {
    let phrase = PHRASES[state.phrase % PHRASES.len()];
    let variants = [
        (TextAnimationVariant::Reveal, "Reveal"),
        (TextAnimationVariant::Cascade, "Cascade"),
        (TextAnimationVariant::Scramble, "Scramble"),
        (TextAnimationVariant::Shimmer, "Shimmer"),
        (TextAnimationVariant::Chromatic, "Chromatic"),
    ];
    let rows: Vec<AnyView<State>> = variants
        .iter()
        .map(|(variant, label)| {
            any(Row(vec![
                any(SizedBox(Some(96.0), None).child(caption(label.to_string()))),
                any(text_animation::<State>(phrase).variant(*variant).size(22.0)),
            ])
            .cross_axis(CrossAxisAlignment::Center))
        })
        .collect();

    let mut body: Vec<AnyView<State>> = Vec::new();
    for row in rows {
        body.push(row);
        body.push(gap(10.0));
    }
    body.push(gap(2.0));
    body.push(knob("Next phrase", |s: &mut State| {
        s.phrase = (s.phrase + 1) % PHRASES.len();
    }));

    demo(
        "text_animation",
        "Five variants of one component. Reveal and Cascade split per grapheme \
         (upstream's word split is not ported); Reveal's and Chromatic's \
         blur halves are dropped — frust's scene has no blur primitive — so \
         both keep only the opacity and translation. Changing the phrase \
         restarts every effect.",
        body,
    )
}

/// The `number` block: both modes over one live value.
fn numbers(state: &State) -> AnyView<State> {
    let value = state.value;
    let revenue = 129_480.0 + value - 48_273.0;

    let roll = specimen(
        "Roll \u{b7} Active users",
        any(number::<State>(value).group(true).size(34.0)),
    );
    let count_up = specimen(
        "CountUp \u{b7} Revenue",
        any(number::<State>(revenue)
            .mode(NumberMode::CountUp)
            .group(true)
            .prefix("$")
            .size(34.0)),
    );
    let padded = specimen(
        "Roll \u{b7} zero-padded, suffixed",
        any(number::<State>(value % 1000.0)
            .pad(4)
            .suffix(" ms")
            .size(34.0)),
    );

    demo(
        "number",
        "Roll is upstream's slot machine, CountUp its value ramp. Grouping is \
         ASCII (en-US), not locale-aware; the ticker's optional blur is not \
         ported; both arm from first paint rather than on a viewport gate.",
        vec![
            any(Row(vec![roll, hgap(48.0), count_up, hgap(48.0), padded])
                .cross_axis(CrossAxisAlignment::Start)),
            gap(14.0),
            any(Row(vec![
                knob("+137", |s: &mut State| s.value += 137.0),
                hgap(8.0),
                knob("+2,480", |s: &mut State| s.value += 2_480.0),
                hgap(8.0),
                knob("\u{2212}1,000", |s: &mut State| {
                    s.value = (s.value - 1_000.0).max(0.0)
                }),
                hgap(8.0),
                knob("Reset", |s: &mut State| s.value = 48_273.0),
            ])),
        ],
    )
}

/// The `animated_badge` block: the live state machine plus every tone at both
/// sizes.
fn badges(state: &State) -> AnyView<State> {
    let (status, label) = BADGE_STATES[state.badge % BADGE_STATES.len()];

    let live = any(Row(vec![
        any(animated_badge::<State>(label)
            .status(status)
            .size(AnimatedBadgeSize::Md)),
        hgap(16.0),
        knob("Advance status", |s: &mut State| {
            s.badge = (s.badge + 1) % BADGE_STATES.len();
        }),
    ])
    .cross_axis(CrossAxisAlignment::Center));

    let tones: Vec<AnyView<State>> = BADGE_STATUSES
        .iter()
        .map(|(status, label)| {
            any(Column(vec![
                any(animated_badge::<State>(*label)
                    .status(*status)
                    .size(AnimatedBadgeSize::Md)),
                gap(6.0),
                any(animated_badge::<State>(*label)
                    .status(*status)
                    .size(AnimatedBadgeSize::Sm)),
                gap(6.0),
                any(animated_badge::<State>(*label)
                    .status(*status)
                    .size(AnimatedBadgeSize::Md)
                    .show_icon(false)),
            ])
            .cross_axis(CrossAxisAlignment::Start))
        })
        .collect();

    demo(
        "animated_badge",
        "A status badge whose icon and label roll on change (Md, Sm, and \
         icon-less below). The roll drops upstream's blur and its \u{2212}8\u{b0} icon \
         rotation; the pill takes its new width immediately rather than \
         gliding, since nothing here layout-animates a container.",
        vec![live, gap(16.0), grid(tones, 3, 24.0)],
    )
}

/// The `loader` block: all seventeen variants at one size.
fn loaders(state: &State) -> AnyView<State> {
    let size = state.loader_size;
    let cells: Vec<AnyView<State>> = LoaderVariant::ALL
        .iter()
        .map(|variant| {
            specimen(
                loader_label(*variant),
                any(SizedBox(Some(size + 24.0), Some(size + 8.0))
                    .child(loader::<State>().variant(*variant).size(size))),
            )
        })
        .collect();

    demo(
        "loader",
        "All seventeen variants. metaballs draws an explicit capsule bridge \
         instead of a gooey SVG filter merge, and morph interpolates each \
         sampled point's radius rather than path data; the ascii sets need the \
         bundled Geist Mono glyphs. Reduced motion keeps a calm opacity pulse \
         here rather than freezing \u{2014} upstream's own rule.",
        vec![
            grid(cells, 6, 16.0),
            gap(16.0),
            any(Row(vec![
                any(caption("Size")),
                hgap(10.0),
                knob("24", |s: &mut State| s.loader_size = 24.0),
                hgap(8.0),
                knob("36", |s: &mut State| s.loader_size = 36.0),
                hgap(8.0),
                knob("48", |s: &mut State| s.loader_size = 48.0),
            ])
            .cross_axis(CrossAxisAlignment::Center)),
        ],
    )
}

/// A loader variant's display name — upstream's own preview labels.
fn loader_label(variant: LoaderVariant) -> &'static str {
    match variant {
        LoaderVariant::Spinner => "Spinner",
        LoaderVariant::Dots => "Dots",
        LoaderVariant::Bars => "Bars",
        LoaderVariant::DotMatrix => "Dot Matrix",
        LoaderVariant::Dither => "Dither",
        LoaderVariant::Ascii => "ASCII",
        LoaderVariant::AsciiLine => "ASCII Line",
        LoaderVariant::AsciiBraille => "ASCII Braille",
        LoaderVariant::AsciiBlocks => "ASCII Blocks",
        LoaderVariant::AsciiBounce => "ASCII Bounce",
        LoaderVariant::Morph => "Morph",
        LoaderVariant::Comet => "Comet",
        LoaderVariant::Scramble => "Scramble",
        LoaderVariant::Metaballs => "Metaballs",
        LoaderVariant::Newton => "Newton",
        LoaderVariant::Helix => "Helix",
        LoaderVariant::Percent => "Percent",
    }
}

/// The `marquee` block: one live ticker, its direction and its edge fade.
fn marquee_demo(state: &State) -> AnyView<State> {
    let direction = state.marquee_direction;
    let fade = state.marquee_fade;
    let items: Vec<AnyView<State>> = LOGOS
        .iter()
        .map(|logo| any(text(logo.to_string()).size(15.0)))
        .collect();

    let height = if direction.is_vertical() { 180.0 } else { 56.0 };
    let track = any(SizedBox(None, Some(height)).child(
        marquee(items)
            .direction(direction)
            .speed(std::time::Duration::from_secs(25))
            .gap(32.0)
            .pause_on_hover(true)
            .fade(fade),
    ));

    let direction_knobs = any(Row(vec![
        any(caption("Direction")),
        hgap(10.0),
        knob("Left", |s: &mut State| {
            s.marquee_direction = MarqueeDirection::Left
        }),
        hgap(8.0),
        knob("Right", |s: &mut State| {
            s.marquee_direction = MarqueeDirection::Right
        }),
        hgap(8.0),
        knob("Up", |s: &mut State| {
            s.marquee_direction = MarqueeDirection::Up
        }),
        hgap(8.0),
        knob("Down", |s: &mut State| {
            s.marquee_direction = MarqueeDirection::Down
        }),
        hgap(16.0),
        knob(
            if fade { "Fade: on" } else { "Fade: off" },
            |s: &mut State| s.marquee_fade = !s.marquee_fade,
        ),
    ])
    .cross_axis(CrossAxisAlignment::Center));

    demo(
        "marquee",
        "Hover the track to pause it (a press does not pause \u{2014} pausing is a \
         hover affordance). The edge fade is an overlay painted in the theme's \
         surface colour rather than upstream's mask, so it is off by default \
         and only exact over a surface-coloured background; items are \
         decorative and take no pointer input.",
        vec![track, gap(14.0), direction_knobs],
    )
}

/// The `theme_toggle` block: four reveals sharing one start origin.
fn theme_toggles(state: &State) -> AnyView<State> {
    let (start, start_label) = TOGGLE_STARTS[state.toggle_start % TOGGLE_STARTS.len()];
    let toggles: Vec<AnyView<State>> = TOGGLE_VARIANTS
        .iter()
        .map(|(variant, label)| {
            specimen(
                label,
                any(theme_toggle::<State>()
                    .variant(*variant)
                    .start(start)
                    .size(32.0)),
            )
        })
        .collect();

    demo(
        "theme_toggle",
        "Each button flips the app's brightness through frust::set_app_theme \
         and reveals its new icon with a different clip. The reveal is scoped \
         to the button's own box: frust has no view-transition seam, so the \
         page behind it simply changes colour. circle-blur fades instead of \
         blurring, and blinds is fixed at four slats rather than a repeating \
         72px tile.",
        vec![
            any(Row(toggles).cross_axis(CrossAxisAlignment::Start)),
            gap(14.0),
            any(Row(vec![
                any(caption(format!("Start \u{b7} {start_label}"))),
                hgap(10.0),
                knob("Next start", |s: &mut State| {
                    s.toggle_start = (s.toggle_start + 1) % TOGGLE_STARTS.len();
                }),
            ])
            .cross_axis(CrossAxisAlignment::Center)),
        ],
    )
}

/// The page's interactive body — see the [module docs](self) for why it is a
/// component rather than an `AppState` field.
struct TextPage;

impl Component for TextPage {
    type State = State;

    fn init(&self) -> State {
        State::default()
    }

    fn build(&self, state: &mut State) -> AnyView<State> {
        any(Column(vec![
            text_effects(state),
            numbers(state),
            badges(state),
            loaders(state),
            marquee_demo(state),
            theme_toggles(state),
        ])
        .cross_axis(CrossAxisAlignment::Start)
        .main_axis(MainAxisAlignment::Start))
    }
}

pub fn page() -> AnyView<AppState> {
    any(Column(vec![
        any(heading("Motion \u{b7} Text")),
        any(SizedBox(None, Some(8.0))),
        any(caption(
            "Text effects, animated numbers, status badges, loaders, the \
             ticker, and the theme toggle \u{2014} beUI's text-and-status motion \
             set, with every registry variant shown.",
        )),
        any(SizedBox(None, Some(24.0))),
        any(component(TextPage)),
    ])
    .cross_axis(CrossAxisAlignment::Start))
}
