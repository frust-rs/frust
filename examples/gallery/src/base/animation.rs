//! `Base`-design animation cases — one per
//! `apps/website/widgets/base/animation/*.mdx` page:
//! `animated-opacity`, `animated-scale`, `pattern-switcher`, `physics`.
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
//! Capturing an actual mid-transition frame needs **two** builds (an initial
//! one, then a rebuild with a different target/key, advanced to a chosen
//! `time_ms`) — outside what today's `Case`/recorder contract can express.
//! That would need a `warm_frames: u8`-shaped field on [`Case`] the recorder
//! rebuilds through before capturing the final frame; this module does not add
//! one (`case.rs` is out of this task's scope) — see g1-05's completion summary
//! for the coordination note to whoever picks that up.
//!
//! `physics` is its own case: `apps/website/widgets/base/animation/physics.mdx`
//! documents `ScrollPhysics` — explicitly "Nothing on this page is a widget"
//! (a strategy object [`scroll_view`](frust_widgets::scroll_view) consults) —
//! so its case renders a `scroll_view` with an installed physics and enough
//! overflowing content to show the surface it governs, rather than the
//! (nonexistent) physics itself.

use frust_core::{AnyView, any};
use frust_widgets::motion::patterns::FadeThrough;
use frust_widgets::motion::switcher::pattern_switcher;
use frust_widgets::motion::{animated_opacity, animated_scale};
use frust_widgets::{Bouncing, Column, container, scroll_view, text};
use peniko::Color;

use crate::case::{Case, Design};

/// Frame backdrop shared by every case in this module — a near-black navy so
/// a light or dark theme's own card accents still read clearly against it.
const BACKDROP: Color = Color::from_rgb8(0x0F, 0x17, 0x2A);

fn animated_opacity_case() -> AnyView<()> {
    any(container(animated_opacity(
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
    .fill(BACKDROP)
    .size_centered(360.0, 240.0))
}

fn animated_scale_case() -> AnyView<()> {
    any(container(animated_scale(
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
    .fill(BACKDROP)
    .size_centered(360.0, 240.0))
}

fn pattern_switcher_case() -> AnyView<()> {
    any(container(pattern_switcher(
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
    .fill(BACKDROP)
    .size_centered(360.0, 240.0))
}

fn physics_row(label: &'static str, tint: Color) -> AnyView<()> {
    any(container(text(label).color(Color::WHITE).size(14.0))
        .fill(tint)
        .radius(8.0)
        .size_centered(320.0, 44.0))
}

fn physics_case() -> AnyView<()> {
    let rows = vec![
        physics_row("Row 1", Color::from_rgb8(0x33, 0x41, 0x55)),
        physics_row("Row 2", Color::from_rgb8(0x3B, 0x82, 0xF6)),
        physics_row("Row 3", Color::from_rgb8(0x33, 0x41, 0x55)),
        physics_row("Row 4", Color::from_rgb8(0x3B, 0x82, 0xF6)),
        physics_row("Row 5", Color::from_rgb8(0x33, 0x41, 0x55)),
        physics_row("Row 6", Color::from_rgb8(0x3B, 0x82, 0xF6)),
    ];
    any(
        container(scroll_view(Column(rows)).physics(Bouncing::new()))
            .fill(BACKDROP)
            .size_centered(360.0, 240.0),
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
        build: animated_opacity_case,
    },
    Case {
        slug: "animated-scale",
        title: "AnimatedScale",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: animated_scale_case,
    },
    Case {
        slug: "pattern-switcher",
        title: "pattern_switcher",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: pattern_switcher_case,
    },
    Case {
        slug: "physics",
        title: "Scroll physics",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: physics_case,
    },
];
