//! Responsive section: proves `frust::WindowMetrics` (tasks 13/14) drives a
//! **structural** layout switch, not a cosmetic one — reading the window's
//! logical size at `build` time and reshaping the same demo content between a
//! single scrolling column (narrow / phone-width windows) and a two-pane
//! master/detail arrangement (wide / tablet-plus windows). This is the
//! reference idiom to copy for any section that needs to adapt to window
//! shape; keep the pattern below (read once at the top of `build`, branch on
//! one named breakpoint, never touch layout after that) rather than inventing
//! a new one per page.
//!
//! # Reading `WindowMetrics`
//!
//! `use_context::<WindowMetrics>()` returns `Option<WindowMetrics>` —
//! deliberately `Option`, not the `expect_context` panicking form, because a
//! shell that has not yet published (or a bare test harness with no shell at
//! all) is a supported state, not a bug. This page falls back to the
//! **narrow** layout whenever the context is absent, exactly the same branch
//! a genuinely narrow window takes, so there is no separate "no metrics" UI
//! to maintain.
//!
//! # The breakpoint
//!
//! [`WIDE_BREAKPOINT_PX`] is the one number this page switches on — see its
//! doc comment for where it comes from. This is a single named constant for
//! *this page*, not a general breakpoint system; a section that needs its own
//! cutoff should declare its own constant the same way rather than reaching
//! for a shared one that does not exist yet.
//!
//! # Context is not reactive
//!
//! `provide_context`/`use_context` is a plain map read (see
//! `frust_core::app::WindowMetrics`'s own doc), not a signal — a shell
//! re-providing a changed `WindowMetrics` does not itself wake anything. What
//! makes this page update live is that the shell only re-provides on an
//! actual resize, and a resize already drives the next rebuild by itself; the
//! read here just observes whatever is current on the rebuild that resize
//! triggers.
//!
//! # Orientation is derived, not platform-sourced
//!
//! [`Orientation`] is always computed from `size` (portrait when `height >=
//! width`, including the exact-square case) — no platform callback carries an
//! orientation enum. On a desktop window this means orientation flips as you
//! resize past square, independent of the width breakpoint below; that is
//! expected behavior of the derivation, not a bug in this page.

use frust::{
    AnyView, Axis, Color, CrossAxisAlignment, EdgeInsets, FlexView, Orientation, Padding, SizedBox,
    Theme, WindowMetrics, any, flexible, inflexible, text, use_context,
};

use crate::CatalogState;

/// The narrow/wide switch point, in logical px (== dp on Android, pt on iOS —
/// `WindowMetrics::size` is already density-independent). Sourced from
/// Material 3's compact/medium window-size-class boundary
/// (<https://m3.material.io/foundations/layout/applying-layout/window-size-classes>),
/// a well-known, already-vetted cutoff rather than a number invented for this
/// page. One constant for this one page — not a general breakpoint API.
const WIDE_BREAKPOINT_PX: f64 = 600.0;

/// One row of the static demo content the master/detail split below
/// reorganizes. The content itself is arbitrary — chosen only to give the two
/// layouts something to reshape — so it is fixed data, not read from
/// `CatalogState`.
struct Article {
    title: &'static str,
    body: &'static str,
}

const ARTICLES: [Article; 4] = [
    Article {
        title: "Retained tree",
        body: "Every build produces a View, diffed against the previous one to \
               patch the retained Widget tree — never a rebuild from scratch.",
    },
    Article {
        title: "Signals, not observers",
        body: "State is RwSignal-backed; a build that reads .get() subscribes \
               its enclosing scope, so only readers of a changed signal rerun.",
    },
    Article {
        title: "Context, not props drilling",
        body: "Theme, WindowInsets, and now WindowMetrics reach build via \
               use_context — a plain re-provided value, not a signal.",
    },
    Article {
        title: "One root, three shells",
        body: "The same Component drives desktop, Android, and iOS; each shell \
               differs only in how it drives rebuild/layout/paint and what it \
               publishes into context.",
    },
];

/// A one-line summary of the current `WindowMetrics` read (or its absence),
/// shown at the top of the page so resizing the window is visibly reflected
/// without needing to inspect anything else.
fn status_line(metrics: Option<WindowMetrics>) -> String {
    match metrics {
        Some(m) => format!(
            "WindowMetrics: {:.0}x{:.0}px @ {:.2}x scale, {} — wide breakpoint is {:.0}px",
            m.size.width,
            m.size.height,
            m.scale,
            orientation_label(m.orientation),
            WIDE_BREAKPOINT_PX,
        ),
        None => "WindowMetrics: not yet published by this shell — using the narrow fallback layout"
            .to_string(),
    }
}

/// A human label for [`Orientation`] (the type itself is `Debug`, but a plain
/// word reads better inline than the derived debug form).
fn orientation_label(orientation: Orientation) -> &'static str {
    match orientation {
        Orientation::Portrait => "Portrait",
        Orientation::Landscape => "Landscape",
    }
}

/// One article's title + body, stacked — the unit both layouts below are
/// built from (the narrow column repeats it top-to-bottom; the wide detail
/// pane repeats it the same way in its own pane).
fn article_body(article: &Article, muted: Color) -> AnyView<CatalogState> {
    any(Padding(
        EdgeInsets::symmetric(0.0, 8.0),
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(any(text(article.title.to_string()).size(13.0))),
                inflexible(any(text(article.body.to_string()).size(11.0).color(muted))),
            ],
        )
        .cross_axis(CrossAxisAlignment::Start),
    ))
}

/// Narrow layout: a single scrolling column with every article's title and
/// body stacked in order — the shape any window under [`WIDE_BREAKPOINT_PX`]
/// gets, including the `WindowMetrics`-absent fallback.
fn narrow_layout(muted: Color) -> AnyView<CatalogState> {
    let items = ARTICLES
        .iter()
        .map(|article| inflexible(article_body(article, muted)))
        .collect();
    any(Padding(
        EdgeInsets::symmetric(16.0, 8.0),
        FlexView::new(Axis::Vertical, items).cross_axis(CrossAxisAlignment::Start),
    ))
}

/// Wide layout's master pane: just the article titles, narrow — a table of
/// contents for the detail pane beside it.
fn master_pane() -> AnyView<CatalogState> {
    let items = ARTICLES
        .iter()
        .map(|article| inflexible(any(text(article.title.to_string()).size(13.0))))
        .collect();
    any(Padding(
        EdgeInsets::all(16.0),
        FlexView::new(Axis::Vertical, items).cross_axis(CrossAxisAlignment::Start),
    ))
}

/// Wide layout's detail pane: every article's full title + body, stacked.
fn detail_pane(muted: Color) -> AnyView<CatalogState> {
    let items = ARTICLES
        .iter()
        .map(|article| inflexible(article_body(article, muted)))
        .collect();
    any(Padding(
        EdgeInsets::all(16.0),
        FlexView::new(Axis::Vertical, items).cross_axis(CrossAxisAlignment::Start),
    ))
}

/// Wide layout: a two-pane master/detail split — a **structural** switch from
/// [`narrow_layout`]'s single column, not merely a padding/font tweak. Left
/// pane (flex 1) is the titles-only master list; right pane (flex 2, wider)
/// is the full detail content, side by side.
fn wide_layout(muted: Color) -> AnyView<CatalogState> {
    any(FlexView::new(
        Axis::Horizontal,
        vec![flexible(1, master_pane()), flexible(2, detail_pane(muted))],
    ))
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(_state: &CatalogState) -> AnyView<CatalogState> {
    // Live theme read for the muted caption color, same pattern every other
    // section page uses (see e.g. `pages::foundations`).
    let theme = use_context::<Theme>().unwrap_or_else(Theme::glyph_baseline);
    let muted = theme.scheme().on_surface_variant;

    // The primitive under test. `None` is a supported state (shell hasn't
    // published yet, or no shell at all in a test harness) — fall back to the
    // narrow layout rather than `expect_context`/panicking.
    let metrics = use_context::<WindowMetrics>();
    let is_wide = metrics
        .map(|m| m.size.width >= WIDE_BREAKPOINT_PX)
        .unwrap_or(false);

    let mut children: Vec<AnyView<CatalogState>> = Vec::new();
    children.push(any(Padding(
        EdgeInsets::symmetric(16.0, 12.0),
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(any(text("Responsive").size(24.0))),
                inflexible(any(text(
                    "Structurally reshapes between a single column and a two-pane \
                     master/detail split, driven by WindowMetrics read live in build. \
                     Resize the window across the breakpoint below to see it switch.",
                )
                .size(11.0)
                .color(muted))),
                inflexible(any(SizedBox(None, Some(6.0)))),
                inflexible(any(text(status_line(metrics)).size(10.5).color(muted))),
                inflexible(any(text(
                    "Orientation is derived from size (portrait when height >= width), \
                     never platform-sourced — on a desktop window it flips as you resize \
                     past square, independent of the breakpoint above. Expected, not a bug.",
                )
                .size(10.0)
                .color(muted))),
            ],
        )
        .cross_axis(CrossAxisAlignment::Start),
    )));

    children.push(if is_wide {
        wide_layout(muted)
    } else {
        narrow_layout(muted)
    });

    any(FlexView::new(
        Axis::Vertical,
        children.into_iter().map(inflexible).collect(),
    )
    .cross_axis(CrossAxisAlignment::Start))
}
