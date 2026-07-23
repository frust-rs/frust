//! Overlays section (`c07`, reference §07): "Only one floating layer at a
//! time" — a confirmation dialog, the signature amber-ring command palette
//! (with a live substring filter), and a bottom-sheet-shaped page pushed
//! straight through the navigator's transparent-page + [`PageTransition::SlideUp`]
//! primitive (no `frust::bottom_sheet`; the reference build's own pane-picker
//! demo, not the M3 catalog widget).
//!
//! # Demo state lives outside `CatalogState` (deliberate)
//!
//! [`page`] is a plain builder re-invoked on every rebuild of the whole app
//! (the section [`pattern_switcher`](frust::motion::switcher::pattern_switcher)
//! in `crate::home_page` re-runs [`pages::current`](crate::pages::current) each
//! time), so a *local* `RwSignal::new(..)` created inline here would reset to
//! its initial value on every single rebuild rather than persist. The command
//! palette's live query and every overlay's last-result caption need to
//! survive across those re-invocations, so — mirroring the Huddle showcase's
//! `workspace_drawer::entrance_progress` precedent — they live behind a
//! self-healing `thread_local!` cache ([`demo_state`]) instead of a new field
//! on the shared [`CatalogState`] (out of this task's scope: `src/pages/overlays.rs`
//! only, see the task file's Hard rules).

use std::cell::Cell;

use frust::glyph::{
    PaletteItem, command_palette, glyph_card, glyph_dialog, show_command_palette, show_glyph_dialog,
};
use frust::{
    AnyView, Axis, ButtonStyle, Color, EdgeInsets, FlexChild, FlexView, Get, GetUntracked,
    NavigatorController, Padding, PageTransition, PopResult, RwSignal, Set, SizedBox, Theme,
    TransitionSpec, any, button, inflexible, text, use_context,
};

use crate::CatalogState;

/// Muted subhead ink (Glyph dark `fg-muted`), matching the reference build's
/// `.subhead` treatment for each overlay's label.
/// Live-theme muted-text role (`on_surface_variant`) (falls back to the Glyph baseline pre-context, mirroring
/// `navigation.rs`'s `accent()` — round-0 review: hardcoded dark-only hexes
/// broke AA under the Light toggle).
fn subhead_ink() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(Theme::glyph_baseline)
        .scheme()
        .on_surface_variant
}
/// Result-caption ink (Glyph dark accent `#ffb627`) — the same amber used for
/// the app title in `crate::header_row`.
/// Live-theme accent-text role (`primary`) (falls back to the Glyph baseline pre-context, mirroring
/// `navigation.rs`'s `accent()` — round-0 review: hardcoded dark-only hexes
/// broke AA under the Light toggle).
fn caption_ink() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(Theme::glyph_baseline)
        .scheme()
        .primary
}
/// Pane-picker row glyph ink (matches [`CAPTION_INK`] — the reference sheet
/// demo's `.sheet-row .g` amber glyph column).
fn sheet_glyph_ink() -> Color {
    caption_ink()
}

/// The palette's + every overlay's last-result caption, bundled so a single
/// `thread_local!` cache (see the [module docs](self)) covers both signals.
/// `RwSignal` is a cheap `Copy` handle, so this whole struct is `Copy` too.
#[derive(Clone, Copy)]
struct OverlaysDemo {
    /// The command palette's controlled query text.
    query: RwSignal<String>,
    /// The last dialog/palette/sheet result, shown under the three buttons.
    caption: RwSignal<String>,
}

thread_local! {
    /// See [`demo_state`] and the [module docs](self)'s "Demo state lives
    /// outside `CatalogState`" note.
    static DEMO: Cell<Option<OverlaysDemo>> = const { Cell::new(None) };
}

/// The cached [`OverlaysDemo`] signals, created once per process and reused on
/// every subsequent `page` call — self-healing like
/// `huddle`'s `workspace_drawer::entrance_progress`: a stale cache entry
/// whose reactive `Owner` was disposed (a headless test re-entering after
/// teardown) is detected via `try_get_untracked()` and transparently
/// recreated rather than trusted blindly.
fn demo_state() -> OverlaysDemo {
    DEMO.with(|cell| {
        if let Some(demo) = cell.get()
            && demo.query.try_get_untracked().is_some()
        {
            return demo;
        }
        let demo = OverlaysDemo {
            query: RwSignal::new(String::new()),
            caption: RwSignal::new(String::new()),
        };
        cell.set(Some(demo));
        demo
    })
}

/// The full, unfiltered command set (reference §07's `cmdk-item` rows plus a
/// few catalog-specific entries), each with a kbd-style trailing hint.
fn all_palette_items() -> Vec<PaletteItem> {
    vec![
        PaletteItem::new("Create new session").hint("↵"),
        PaletteItem::new("New session from layout…").hint("⇧↵"),
        PaletteItem::new("Rename current session").hint("⌘R"),
        PaletteItem::new("Toggle brightness").hint("⌘B"),
        PaletteItem::new("Toggle reduced motion").hint("⌘M"),
        PaletteItem::new("Jump to Foundations").hint("⌘1"),
        PaletteItem::new("Jump to Feedback").hint("⌘4"),
        PaletteItem::new("Open bottom sheet demo").hint("⌘K"),
    ]
}

/// The live substring filter driving `on_query` — the app's job (v1: the
/// palette itself has no fuzzy engine, see `frust::glyph::command_palette`'s
/// module docs). Case-insensitive; an empty query returns every item.
fn filter_items(query: &str) -> Vec<PaletteItem> {
    if query.is_empty() {
        return all_palette_items();
    }
    let needle = query.to_lowercase();
    all_palette_items()
        .into_iter()
        .filter(|item| item.label.to_lowercase().contains(&needle))
        .collect()
}

/// A muted section subhead (mirrors the reference build's `.subhead`).
fn subhead(label: &str) -> AnyView<CatalogState> {
    any(text(label.to_string()).size(11.5).color(subhead_ink()))
}

/// The last-result caption row (empty until a dialog/palette/sheet closes).
fn caption_view(caption: &str) -> AnyView<CatalogState> {
    any(text(caption.to_string()).size(12.5).color(caption_ink()))
}

/// Wire the "Open dialog" button: `show_glyph_dialog` with the reference
/// build's own confirmation copy (§07's "Revoke observer-token?"), a ghost
/// Cancel + a danger Revoke action. Both actions pop immediately (bypassing
/// the dialog's own exit animation — the same immediacy `crate::glyph::dialog`'s
/// tests document for an action tap); the scrim/Escape path still plays the
/// widget's staged exit. The result lands in [`OverlaysDemo::caption`].
fn open_dialog_button(
    demo: OverlaysDemo,
    nav: NavigatorController<CatalogState>,
) -> FlexChild<CatalogState> {
    inflexible(button("Open dialog", move |_: &mut CatalogState| {
        let cancel_nav = nav.clone();
        let confirm_nav = nav.clone();
        show_glyph_dialog(
            &nav,
            move || {
                glyph_dialog()
                    .title("Revoke observer-token?")
                    .body("Any device using this token loses access immediately. This can't be undone.")
                    .action(any(button("Cancel", {
                        let cancel_nav = cancel_nav.clone();
                        move |_: &mut CatalogState| cancel_nav.pop()
                    })
                    .style(ButtonStyle::Ghost)
                    .small()))
                    .action(any(button("Revoke token", {
                        let confirm_nav = confirm_nav.clone();
                        move |_: &mut CatalogState| confirm_nav.pop_with_result(PopResult::of(true))
                    })
                    .style(ButtonStyle::Danger)
                    .small()))
            },
            move |_: &mut CatalogState, result: PopResult| {
                let revoked = result.take::<bool>().unwrap_or(false);
                demo.caption.set(if revoked {
                    "Dialog: revoked observer-token".to_string()
                } else {
                    "Dialog: cancelled".to_string()
                });
            },
        );
    }))
}

/// Wire the "Command palette" button: `show_command_palette` over the
/// filtered [`all_palette_items`], streaming `on_query` into
/// [`OverlaysDemo::query`] (a write the palette's own re-invoked `build`
/// closure reads back, re-filtering live) and popping with the selected
/// index on `on_select` (the same immediate-pop-bypasses-exit-animation
/// pattern the dialog's actions use).
fn open_palette_button(
    demo: OverlaysDemo,
    nav: NavigatorController<CatalogState>,
) -> FlexChild<CatalogState> {
    inflexible(button("Command palette", move |_: &mut CatalogState| {
        demo.query.set(String::new());
        let select_nav = nav.clone();
        show_command_palette(
            &nav,
            move || {
                let query = demo.query.get();
                let items = filter_items(&query);
                let select_nav = select_nav.clone();
                command_palette(
                    items,
                    move |_: &mut CatalogState, next_query: String| {
                        demo.query.set(next_query);
                    },
                    move |_: &mut CatalogState, index: usize| {
                        select_nav.pop_with_result(PopResult::of(index));
                    },
                )
                .query(query)
                .placeholder("Type a command or search…")
            },
            move |_: &mut CatalogState, result: PopResult| {
                let caption = match result.take::<usize>() {
                    Some(index) => {
                        let query = demo.query.get_untracked();
                        match filter_items(&query).get(index) {
                            Some(item) => format!("Palette: {}", item.label),
                            None => "Palette: dismissed".to_string(),
                        }
                    }
                    None => "Palette: dismissed".to_string(),
                };
                demo.caption.set(caption);
            },
        );
    }))
}

/// The pane-picker rows from the motion reference build's own bottom-sheet
/// demo (glyph-motion.html §06) — reused here as content, not the transition
/// itself (`c08` owns the motion-pattern showcase).
const SHEET_ROWS: [(&str, &str); 4] = [
    ("▣", "Split pane right"),
    ("▤", "Split pane down"),
    ("⛶", "Zoom current pane"),
    ("×", "Close pane"),
];

/// One pane-picker row: a small amber glyph marker + its label.
fn sheet_row(glyph: &str, label: &str) -> FlexChild<CatalogState> {
    inflexible(FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(text(glyph.to_string()).size(13.0).color(sheet_glyph_ink())),
            inflexible(SizedBox(Some(12.0), None)),
            inflexible(text(label.to_string()).size(12.5)),
        ],
    ))
}

/// The sheet page's body: the four pane-picker rows plus a Close button that
/// pops the navigator (the task's "close button pops" requirement).
fn sheet_body(nav: NavigatorController<CatalogState>) -> AnyView<CatalogState> {
    let mut children: Vec<FlexChild<CatalogState>> = SHEET_ROWS
        .iter()
        .map(|(g, label)| sheet_row(g, label))
        .collect();
    children.push(inflexible(SizedBox(None, Some(8.0))));
    children.push(inflexible(
        button("Close", move |_: &mut CatalogState| nav.pop())
            .style(ButtonStyle::Ghost)
            .small(),
    ));
    any(FlexView::new(Axis::Vertical, children))
}

/// Wire the "Bottom sheet" button: `push_transparent_for_result` with
/// [`PageTransition::SlideUp`] and a `glyph_card`-shaped page (the task's
/// "card-ish sheet page") holding [`sheet_body`] — the reference build's
/// pane-picker sheet demo, driven through the raw navigator primitive rather
/// than the M3 `frust::bottom_sheet` catalog widget (this app has no
/// Glyph-styled sheet chrome of its own to reach for).
fn open_sheet_button(
    demo: OverlaysDemo,
    nav: NavigatorController<CatalogState>,
) -> FlexChild<CatalogState> {
    inflexible(button("Bottom sheet", move |_: &mut CatalogState| {
        let card_nav = nav.clone();
        nav.push_transparent_for_result(
            move || {
                any(glyph_card::<CatalogState>()
                    .title(text("Pane picker".to_string()).size(13.5))
                    .desc(sheet_body(card_nav.clone())))
            },
            TransitionSpec::duration(PageTransition::SlideUp),
            move |_: &mut CatalogState, _result: PopResult| {
                demo.caption.set("Sheet: closed".to_string());
            },
        );
    }))
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &CatalogState) -> AnyView<CatalogState> {
    let demo = demo_state();
    let caption = demo.caption.get();
    let nav = state.nav.clone();

    let mut children: Vec<FlexChild<CatalogState>> = vec![
        inflexible(
            text(
                "Only one floating layer at a time. The command palette is the fastest path \
                 through the app for anyone comfortable typing."
                    .to_string(),
            )
            .size(12.5),
        ),
        inflexible(SizedBox(None, Some(20.0))),
        inflexible(subhead("confirmation modal")),
        inflexible(SizedBox(None, Some(8.0))),
        open_dialog_button(demo, nav.clone()),
        inflexible(SizedBox(None, Some(24.0))),
        inflexible(subhead("command palette")),
        inflexible(SizedBox(None, Some(8.0))),
        open_palette_button(demo, nav.clone()),
        inflexible(SizedBox(None, Some(24.0))),
        inflexible(subhead("bottom sheet")),
        inflexible(SizedBox(None, Some(8.0))),
        open_sheet_button(demo, nav),
    ];

    if !caption.is_empty() {
        children.push(inflexible(SizedBox(None, Some(20.0))));
        children.push(inflexible(caption_view(&caption)));
    }

    any(Padding(
        EdgeInsets::all(16.0),
        FlexView::new(Axis::Vertical, children),
    ))
}
