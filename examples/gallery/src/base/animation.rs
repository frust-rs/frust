//! `Base`-design animation cases — one per
//! `apps/website/widgets/base/animation/*.mdx` page:
//! `animated-opacity`, `animated-scale`, `pattern-switcher`, `physics`,
//! `card-switcher`.
//!
//! # Why every frame here is a settled/steady-state frame
//!
//! [`Case::build`] is a plain `fn() -> `[`frust_core::AnyView`]`<()>`, called
//! fresh on every rebuild (see the [crate docs](crate) and `README.md`) — there
//! is no *previous* `View` a rebuild diffs a changed target/key against, only a
//! brand-new tree every pass. That matters for every wrapper in this module:
//!
//! - [`AnimatedOpacityWidget`](frust_widgets::motion::animated::AnimatedOpacityWidget)/
//!   [`AnimatedScaleWidget`](frust_widgets::motion::animated::AnimatedScaleWidget)
//!   seed their retained animation at `ImplicitAnim::new(target)` on `build` —
//!   *already settled* at `target` (Flutter's "no animate-in on first mount"
//!   convention) — so a single-build case renders the wrapper at its resting
//!   value for **any** [`Case::time_ms`]; there is no mid-retarget frame a
//!   single build can ever show.
//! - [`PatternSwitcherWidget`](frust_widgets::motion::switcher::PatternSwitcherWidget)
//!   likewise starts with `exiting: None` on `build` — a changed-`key`
//!   transition is staged only on a *rebuild* whose `key` differs from the
//!   previous pass's — so a single build always paints the plain steady-state
//!   child, with no crossfade/slide bracket at all.
//!
//! Capturing an actual mid-transition frame needs **two builds with different
//! inputs** (an initial one, then a rebuild whose target/key has changed,
//! advanced to a chosen `time_ms`) — and that is still outside what the
//! `Case`/recorder contract can express, for a reason the recorder cannot
//! reach.
//!
//! The recorder *does* now paint warm passes before the captured one
//! ([`Case::warm_frames`], derived from a non-zero [`Case::time_ms`] rather
//! than declared as a field — no `warm_frames` field exists on [`Case`], and
//! one was deliberately not added; `crates/frust-testing/src/frame.rs`'s
//! `FrameSpec::warm_frames` is the recorder-side half). A warm pass is a full
//! rebuild -> layout -> paint, so it is a second *build* as well as a second
//! *advance*. What it is not is a build with different inputs: [`Case::build`]
//! is a pure `fn() -> `[`frust_core::AnyView`]`<()>`, so every pass returns the
//! same target and the same key, and the retarget/key-change these three
//! wrappers stage on is never staged. Warming therefore buys them exactly
//! nothing, and all three keep [`Case::DEFAULT_TIME_MS`] — measured, not
//! assumed: each was recorded at `time_ms: 400` with the warm pass active and
//! produced a **byte-identical** PNG in both variants.
//!
//! (The two cases the warm pass does move both live in
//! [`crate::beui`] — a modal whose lanes are `Presence`-staged, and a balance
//! on a per-grapheme cascade. Both are staged by *elapsed time* on a single
//! build, which is the shape warming addresses.)
//!
//! `physics` is its own case: `apps/website/widgets/base/animation/physics.mdx`
//! documents `ScrollPhysics` — explicitly "Nothing on this page is a widget"
//! (a strategy object [`scroll_view`](frust_widgets::scroll_view) consults) —
//! so its case renders a `scroll_view` with an installed physics and enough
//! overflowing content to show the surface it governs, rather than the
//! (nonexistent) physics itself.
//!
//! `card-switcher` is the one case here whose single build *is* its settled
//! frame for a reason of its own: [`card_switcher`] mounted already `open`
//! does not animate in (its stage ramp is seeded at the mount state), so the
//! recorded frame at [`Case::DEFAULT_TIME_MS`] is the settled open stage, with
//! no warm pass and no entry in `WARM_PASS_OPT_INS`.
//!
//! Every case takes its frame from [`super::framed`], which sizes without
//! filling, and keeps its animated card smaller than that frame — so the
//! recorder's per-variant `surface` clear stays visible around it (see
//! [`super`]'s "The variant has to reach the pixels").

use frust_core::View;
use frust_widgets::motion::patterns::FadeThrough;
use frust_widgets::motion::switcher::pattern_switcher;
use frust_widgets::motion::{animated_opacity, animated_scale, card_switcher, peer_card};
use frust_widgets::{
    Align, Alignment, Bouncing, Column, CrossAxisAlignment, SizedBox, button, column, container,
    scroll_view, text,
};
use kurbo::Size;
use peniko::Color;

use super::{framed, framed_in};
use crate::case::{Case, Design, case_build};

/// The viewport the `physics` case's `scroll_view` is boxed into. A
/// scrollable viewport measures its content against a LOOSE cross axis and
/// then start-aligns it, so a row narrower than the viewport would sit off
/// centre — the width here matches the rows' own, and the box is what keeps
/// the variant's cleared surface visible around the scrolling area. The
/// height is short enough that the sixth row overflows, which is the point
/// of a physics case.
const SCROLL_VIEWPORT: (f64, f64) = (280.0, 196.0);

fn animated_opacity_case() -> impl View<()> {
    framed(animated_opacity(
        0.55,
        container(
            text("AnimatedOpacity — target 0.55")
                .color(Color::WHITE)
                .size(16.0),
        )
        .fill(Color::from_rgb8(0x3B, 0x82, 0xF6))
        .radius(16.0)
        .size_centered(300.0, 160.0),
    ))
}

fn animated_scale_case() -> impl View<()> {
    framed(animated_scale(
        1.15,
        container(
            text("AnimatedScale — target 1.15x")
                .color(Color::WHITE)
                .size(16.0),
        )
        .fill(Color::from_rgb8(0x22, 0xC5, 0x5E))
        .radius(16.0)
        .size_centered(240.0, 140.0),
    ))
}

fn pattern_switcher_case() -> impl View<()> {
    framed(pattern_switcher(
        "gallery-card",
        FadeThrough,
        container(
            text("PatternSwitcher — steady state")
                .color(Color::WHITE)
                .size(16.0),
        )
        .fill(Color::from_rgb8(0xA8, 0x55, 0xF7))
        .radius(16.0)
        .size_centered(280.0, 150.0),
    ))
}

fn physics_row(label: &'static str, tint: Color) -> impl View<()> {
    container(text(label).color(Color::WHITE).size(14.0))
        .fill(tint)
        .radius(8.0)
        .size_centered(280.0, 44.0)
}

fn physics_case() -> impl View<()> {
    let rows = vec![
        physics_row("Row 1", Color::from_rgb8(0x33, 0x41, 0x55)),
        physics_row("Row 2", Color::from_rgb8(0x3B, 0x82, 0xF6)),
        physics_row("Row 3", Color::from_rgb8(0x33, 0x41, 0x55)),
        physics_row("Row 4", Color::from_rgb8(0x3B, 0x82, 0xF6)),
        physics_row("Row 5", Color::from_rgb8(0x33, 0x41, 0x55)),
        physics_row("Row 6", Color::from_rgb8(0x3B, 0x82, 0xF6)),
    ];
    framed(
        SizedBox(Some(SCROLL_VIEWPORT.0), Some(SCROLL_VIEWPORT.1))
            .child(scroll_view(Column(rows)).physics(Bouncing::new())),
    )
}

/// The `card-switcher` frame: taller than [`Case::DEFAULT_SIZE`] so the live
/// page reads as a phone screen and its shrunk card keeps a page's
/// proportions.
const SWITCHER_FRAME: Size = Size::new(360.0, 320.0);

/// Ink for the `card-switcher` page faces, painted on their fixed swatches.
const FACE_INK: Color = Color::from_rgb8(0x1F, 0x29, 0x37);

/// The `card-switcher` pages, `(title, subtitle, swatch)`, in strip order.
const FACES: [(&str, &str, Color); 3] = [
    ("Inbox", "3 unread", Color::from_rgb8(0xBF, 0xDB, 0xFE)),
    (
        "Calendar",
        "Stand-up at 10:00",
        Color::from_rgb8(0xBB, 0xF7, 0xD0),
    ),
    (
        "Notes",
        "Last edited today",
        Color::from_rgb8(0xFE, 0xF3, 0xC7),
    ),
];

/// The strip position of the live page: the middle one, so a peer peeks in on
/// each side.
const LIVE_FACE: usize = 1;

/// One page face: its title and subtitle centred on its swatch. Faces fill
/// whatever they are given, which for the stage is the full frame.
fn face(index: usize) -> impl View<()> {
    let (title, subtitle, swatch) = FACES[index];
    container(Align(
        Alignment::CENTER,
        column()
            .child(text(title).size(22.0).color(FACE_INK))
            .child(SizedBox(None, Some(6.0)))
            .child(text(subtitle).size(14.0).color(FACE_INK))
            .cross_axis(CrossAxisAlignment::Center),
    ))
    .fill(swatch)
}

/// The live page: the middle face plus the button that would open the
/// switcher (the live twin in `crate::interactive::base` wires it).
fn live_page() -> impl View<()> {
    let (title, subtitle, swatch) = FACES[LIVE_FACE];
    container(Align(
        Alignment::CENTER,
        column()
            .child(text(title).size(22.0).color(FACE_INK))
            .child(SizedBox(None, Some(6.0)))
            .child(text(subtitle).size(14.0).color(FACE_INK))
            .child(SizedBox(None, Some(16.0)))
            .child(button("Switch page", |_: &mut ()| {}))
            .cross_axis(CrossAxisAlignment::Center),
    ))
    .fill(swatch)
}

/// `card_switcher`, open: the live page shrunk into the centred card over the
/// scrim, with a peer card peeking in on each side.
fn card_switcher_case() -> impl View<()> {
    framed_in(
        SWITCHER_FRAME,
        SizedBox(Some(SWITCHER_FRAME.width), Some(SWITCHER_FRAME.height)).child(
            card_switcher(true, live_page())
                .peers([peer_card(face(0)), peer_card(face(2))])
                .live_index(LIVE_FACE),
        ),
    )
}

/// This module's slice of the registry [`crate::cases`] concatenates.
pub const CASES: &[Case] = &[
    Case {
        slug: "animated-opacity",
        title: "AnimatedOpacity",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: case_build!(animated_opacity_case),
    },
    Case {
        slug: "animated-scale",
        title: "AnimatedScale",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: case_build!(animated_scale_case),
    },
    Case {
        slug: "pattern-switcher",
        title: "pattern_switcher",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: case_build!(pattern_switcher_case),
    },
    Case {
        slug: "physics",
        title: "Scroll physics",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: case_build!(physics_case),
    },
    Case {
        slug: "card-switcher",
        title: "card_switcher",
        size: SWITCHER_FRAME,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: case_build!(card_switcher_case),
    },
];
