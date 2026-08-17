//! Navigation section — **filled by `c05`**: a standalone tabs demo with a
//! sliding indicator, a controlled segmented control, a breadcrumb whose taps
//! update a caption, two `glyph_nav_bar` strips (CHAR items vs VECTOR items —
//! the box-drawing/tofu coverage story), and three avatar size/accent
//! variants.
//!
//! # Local demo state (deliberately not `CatalogState`)
//!
//! Every controlled demo below (the tab index, the segment index, the
//! breadcrumb caption, the two nav-bar selections) is genuinely page-local —
//! nothing any other section needs to read or drive — so it is **not** added
//! to [`crate::CatalogState`] (the page-fn contract fixes that struct;
//! growing it per section would defeat the point of a shared bundle). Instead
//! each demo caches its own [`RwSignal`] in a `thread_local!` `Cell`, created
//! once per process by [`demo_signal`] and self-healing if its owning
//! `Owner` was ever disposed and recreated — mirrors huddle's
//! `workspace_drawer::entrance_progress`/`thread::composer_for` precedent
//! (see either's module docs for the full rationale).
//!
//! # Char-vs-icon nav bar coverage story
//!
//! The reference build's bottom-nav icons are geometric-shape glyphs (▣ ◎ ◆
//! ◉) the bundled Glyph fonts (Space Mono / IBM Plex Mono) do **not** cover —
//! only Box Drawing (U+2500–257F) ships in the bundled coverage. A `Char`
//! item outside that coverage falls through to the platform's system-font
//! fallback: broad on desktop/Android, narrow on iOS, where an uncovered
//! codepoint renders as a tofu box (found on-device 2026-07-23, per
//! `frust_glyph::navbar`'s `NavItemGlyph` doc). The two strips below
//! demonstrate both ends of that story: the top bar uses `┌ ─ ╳`
//! (real bundled-font Box Drawing coverage, safe everywhere), the bottom bar
//! uses `glyph_nav_item_icon` — app-local **vector** glyphs (square, diamond,
//! a ringed dot) that render identical pixels on every platform, the way any
//! icon needing the Geometric Shapes look should ship.
//!
//! ## Why `IconSource`, not a hand-built `kurbo::BezPath`
//!
//! Huddle's `Tab::glyph_icon` (the task's cited precedent) builds its vector
//! icons from a `BezPath` directly — reached, like every custom widget now,
//! through `frust::authoring` (`docs/CODE_STANDARDS.md`'s State & Reactivity
//! Conventions). That is available here too, but this catalog deliberately
//! does **not** hand-roll a vector icon at all. The
//! same deterministic-vector-path outcome is reachable with no extra
//! dependency at all: [`frust::IconSource`] — the exact `{ d: &'static str,
//! design: f64 }` shape `scripts/gen_icons.py` emits for the vendored
//! Material Symbols set (`frust_widgets::icon`'s module docs) — implements
//! `Into<IconData>`, and its `d` field is a plain SVG path-data string
//! (`kurbo::BezPath::from_svg` parses it at `resolve()` time, entirely inside
//! `frust-widgets`). [`SQUARE_ICON`]/[`DIAMOND_ICON`]/[`RINGED_DOT_ICON`]
//! below hand-author that same `d` string as this file's own module-level
//! consts — no `kurbo` import needed here, matching every other page's
//! dependency footprint.

use std::cell::Cell;

use frust::{
    AnyView, Color, Column, EdgeInsets, Get, IconSource, Padding, Row, RwSignal, Set, SizedBox,
    Theme, WithUntracked, any, text, use_context,
};

use crate::CatalogState;

// ---- Section chrome ---------------------------------------------------

/// Section-title accent — the live theme's `primary` role (falls back to the
/// Glyph baseline if no theme context has been provided yet, mirroring
/// huddle's `use_context::<Theme>().unwrap_or_else(Theme::m3_baseline)`
/// precedent — see e.g. `examples/huddle/src/lib.rs`).
fn accent() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_glyph::baseline)
        .scheme()
        .primary
}

/// A section heading: label + a little breathing room below it.
fn heading(label: &str) -> AnyView<CatalogState> {
    any(Column(vec![
        any(text(label.to_string()).size(15.0).color(accent())),
        any(SizedBox(Some(0.0), Some(8.0))),
    ]))
}

/// Vertical gap between demo blocks.
fn gap(px: f64) -> AnyView<CatalogState> {
    any(SizedBox(Some(0.0), Some(px)))
}

// ---- Local demo state (see module docs) --------------------------------

thread_local! {
    static TABS_SEL: Cell<Option<RwSignal<usize>>> = const { Cell::new(None) };
    static SEG_SEL: Cell<Option<RwSignal<usize>>> = const { Cell::new(None) };
    static CRUMB_CAPTION: Cell<Option<RwSignal<String>>> = const { Cell::new(None) };
    static CHAR_NAV_SEL: Cell<Option<RwSignal<usize>>> = const { Cell::new(None) };
    static ICON_NAV_SEL: Cell<Option<RwSignal<usize>>> = const { Cell::new(None) };
}

/// Fetch (or lazily create) a page-local demo signal cached in a
/// `thread_local!` cell. Self-healing: a disposed signal (its owning `Owner`
/// gone — a reused test thread, say) is recreated rather than handed back to
/// panic on the next get/set, matching huddle's
/// `workspace_drawer::entrance_progress` precedent (see its module docs).
fn demo_signal<T: Send + Sync + 'static>(
    cell: &'static std::thread::LocalKey<Cell<Option<RwSignal<T>>>>,
    init: impl FnOnce() -> T,
) -> RwSignal<T> {
    cell.with(|cell| {
        if let Some(sig) = cell.get()
            && sig.try_with_untracked(|_| ()).is_some()
        {
            return sig;
        }
        let sig = RwSignal::new(init());
        cell.set(Some(sig));
        sig
    })
}

const DEFAULT_CRUMB_CAPTION: &str = "tap a crumb above";

fn tabs_selected() -> RwSignal<usize> {
    demo_signal(&TABS_SEL, || 0)
}
fn segmented_selected() -> RwSignal<usize> {
    demo_signal(&SEG_SEL, || 0)
}
fn breadcrumb_caption() -> RwSignal<String> {
    demo_signal(&CRUMB_CAPTION, || DEFAULT_CRUMB_CAPTION.to_string())
}
fn char_nav_selected() -> RwSignal<usize> {
    demo_signal(&CHAR_NAV_SEL, || 0)
}
fn icon_nav_selected() -> RwSignal<usize> {
    demo_signal(&ICON_NAV_SEL, || 0)
}

// ---- App-local vector icons (square / diamond / ringed dot) -----------
//
// Hand-authored SVG path data over a 10x10 design box, in the exact
// `IconSource { d, design }` shape `frust::icons` generates for the vendored
// Material Symbols set (see the module docs' "Why `IconSource`" section).
// Nonzero-fill rings pair an outer subpath with an oppositely-wound inner
// subpath — the same ring-via-opposite-winding technique
// `frust_glyph::navbar`'s huddle-side precedent uses, just expressed
// as SVG path data instead of `kurbo::BezPath` calls.

/// A square outline (outer ring minus an inner square hole).
const SQUARE_ICON: IconSource = IconSource {
    d: "M 0.5 0.5 L 9.5 0.5 L 9.5 9.5 L 0.5 9.5 Z M 1.8 1.8 L 1.8 8.2 L 8.2 8.2 L 8.2 1.8 Z",
    design: 10.0,
};

/// A filled diamond.
const DIAMOND_ICON: IconSource = IconSource {
    d: "M 5 0.2 L 9.8 5 L 5 9.8 L 0.2 5 Z",
    design: 10.0,
};

/// A circle outline with a filled center dot (two arcs per ring/dot circle —
/// the standard "two semicircle arcs" SVG circle idiom, opposite sweep flags
/// between the outer ring and its inner hole to keep the hole's winding
/// opposite the outer's under the nonzero fill rule).
const RINGED_DOT_ICON: IconSource = IconSource {
    d: "M 0.4 5 A 4.6 4.6 0 0 1 9.6 5 A 4.6 4.6 0 0 1 0.4 5 Z \
        M 1.8 5 A 3.2 3.2 0 0 0 8.2 5 A 3.2 3.2 0 0 0 1.8 5 Z \
        M 3.4 5 A 1.6 1.6 0 0 1 6.6 5 A 1.6 1.6 0 0 1 3.4 5 Z",
    design: 10.0,
};

// ---- Demo blocks --------------------------------------------------------

/// A standalone 3-tab strip (moving underline indicator) — independent of
/// the shell's own section tabs.
fn tabs_demo() -> AnyView<CatalogState> {
    let sig = tabs_selected();
    let selected = sig.get();
    let labels = vec![
        "Overview".to_string(),
        "Activity".to_string(),
        "Settings".to_string(),
    ];
    any(frust_glyph::tabs(
        labels,
        selected,
        move |_state: &mut CatalogState, idx: usize| sig.set(idx),
    ))
}

/// A controlled 3-segment pill group.
fn segmented_demo() -> AnyView<CatalogState> {
    let sig = segmented_selected();
    let selected = sig.get();
    let segments = vec![
        "List".to_string(),
        "Grid".to_string(),
        "Compact".to_string(),
    ];
    any(frust_glyph::segmented_control(
        segments,
        selected,
        move |_state: &mut CatalogState, idx: usize| sig.set(idx),
    ))
}

/// A 3-crumb breadcrumb (last crumb current, non-link); tapping a link crumb
/// updates the caption beneath it.
fn breadcrumb_demo() -> AnyView<CatalogState> {
    let caption_sig = breadcrumb_caption();
    let crumbs = vec![
        frust_glyph::crumb::<CatalogState>("catalog").on_tap(move |_s: &mut CatalogState| {
            caption_sig.set("→ catalog".to_string());
        }),
        frust_glyph::crumb::<CatalogState>("sections").on_tap(move |_s: &mut CatalogState| {
            caption_sig.set("→ sections".to_string());
        }),
        frust_glyph::crumb::<CatalogState>("navigation"),
    ];
    let caption = caption_sig.get();
    any(Column(vec![
        any(frust_glyph::breadcrumb(crumbs)),
        gap(6.0),
        any(text(caption).size(11.0)),
    ]))
}

/// The CHAR-item nav bar: box-drawing glyphs the bundled fonts actually cover
/// (see the module docs' coverage story).
fn char_nav_bar_demo() -> AnyView<CatalogState> {
    let sig = char_nav_selected();
    let selected = sig.get();
    let items = vec![
        frust_glyph::glyph_nav_item("┌", "frame"),
        frust_glyph::glyph_nav_item("─", "stream"),
        frust_glyph::glyph_nav_item("╳", "close"),
    ];
    any(frust_glyph::glyph_nav_bar(
        items,
        selected,
        move |_state: &mut CatalogState, idx: usize| sig.set(idx),
    ))
}

/// The VECTOR-item nav bar: deterministic app-local icon paths (see the
/// module docs' coverage story).
fn icon_nav_bar_demo() -> AnyView<CatalogState> {
    let sig = icon_nav_selected();
    let selected = sig.get();
    let items = vec![
        frust_glyph::glyph_nav_item_icon(SQUARE_ICON, "square"),
        frust_glyph::glyph_nav_item_icon(DIAMOND_ICON, "diamond"),
        frust_glyph::glyph_nav_item_icon(RINGED_DOT_ICON, "dot"),
    ];
    any(frust_glyph::glyph_nav_bar(
        items,
        selected,
        move |_state: &mut CatalogState, idx: usize| sig.set(idx),
    ))
}

/// Three avatars: default size/accent, a larger custom accent, and a small
/// override-everything variant.
fn avatars_demo() -> AnyView<CatalogState> {
    any(Row(vec![
        any(frust_glyph::avatar("ed").size(28.0)),
        any(SizedBox(Some(12.0), None)),
        any(frust_glyph::avatar("mk").size(40.0)),
        any(SizedBox(Some(12.0), None)),
        any(frust_glyph::avatar("ai")
            .size(52.0)
            .accent(Color::from_rgb8(0x39, 0x49, 0xAB))),
    ]))
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(_state: &CatalogState) -> AnyView<CatalogState> {
    any(Padding(
        EdgeInsets::all(16.0),
        Column(vec![
            heading("Tabs"),
            tabs_demo(),
            gap(20.0),
            heading("Segmented Control"),
            segmented_demo(),
            gap(20.0),
            heading("Breadcrumb"),
            breadcrumb_demo(),
            gap(20.0),
            heading("Nav Bar — chars vs. icons"),
            any(text(
                "Box-drawing chars (bundled-font coverage) above; \
                     deterministic vector icons (no font-fallback dependency) below."
                    .to_string(),
            )
            .size(11.0)),
            gap(8.0),
            char_nav_bar_demo(),
            gap(8.0),
            icon_nav_bar_demo(),
            gap(20.0),
            heading("Avatars"),
            avatars_demo(),
        ]),
    ))
}
