//! Shortcuts section: the keyboard-shortcut registry, modal key scoping and
//! Tab traversal, driven by real chords.
//!
//! # What is bound
//!
//! | Chord       | Scope                        | Effect                                 |
//! |-------------|------------------------------|----------------------------------------|
//! | `Mod+K`     | [`ShortcutScope::AppGlobal`] | opens the command palette              |
//! | `?`         | [`ShortcutScope::AppGlobal`] | opens the command palette too          |
//! | `Mod+B`     | [`ShortcutScope::Local`]     | toggles the side pane                  |
//! | `F2`        | [`ShortcutScope::Local`]     | toggles the `One*` rename of tab one   |
//! | `1`/`2`/`3` | [`ShortcutScope::Local`]     | selects tab one, two or three          |
//!
//! `Mod` is Command on macOS/iOS and Ctrl elsewhere. `Mod+B` stays the pane
//! toggle, chosen before function keys existed; `F2` is the page's function-key
//! chord. `?` is a symbol key, so Shift does not change whether it matches.
//!
//! # The palette
//!
//! An [`overlay_portal`] anchored to the page header, mounted while
//! [`ShortcutsState::palette_open`] is set, with [`OverlayKeys::Modal`]: while
//! it is open the page's chords are masked, and `Escape` closes it through
//! [`on_escape`](frust::OverlayPortalView::on_escape) whether or not the filter
//! is focused. It holds a text input (the filter) and a list of the live
//! shortcut hints, each labelled with [`Chord::display`]. The hints come from
//! `BuildCtx::shortcut_hints()` — the *previous* frame's declarations in paint
//! order — read by [`WithHints`], a one-view adapter that hands them to a
//! closure on every build and rebuild.
//!
//! # Focus and precedence
//!
//! A row of baseline controls (Button, Checkbox, Radio, Slider, TextInput) is
//! the first run of Tab stops on the page, so Tab / Shift+Tab walk it (the
//! baseline controls paint no focus cue; the field's border is the visible
//! stop). A focused text input consumes typed text, declines Tab and every
//! chord it does not decode (so `Mod+K`, `Mod+B` and `F2` still fire while it
//! has focus). A bare Escape blurs it and is declined, so an open palette closes
//! on the first press even with its filter focused. Bare `1`/`2`/`3`, `?` and
//! typed text reach the registry only when no focused widget takes them. A press
//! on a baseline control keeps keyboard focus on it, so Tab continues from
//! there. The readout line names the last consumer; `tests/shortcuts.rs` pins
//! the whole matrix.

use std::cell::RefCell;
use std::rc::Rc;

use frust::authoring::{BuildCtx, ChangeFlags};
use frust::{
    ButtonStyle, Chord, Color, EdgeInsets, Either, Get, OverlayKeys, OverlayPlacement, OverlaySide,
    Padding, RwSignal, Set, ShortcutHint, ShortcutScope, SizedBox, Theme, Update, View, button,
    checkbox, column, container, overlay_portal, radio, row, shortcuts, slider, text, text_input,
    use_context,
};

use crate::PlaygroundState;

/// The tab labels, selected by the `1`/`2`/`3` chords.
pub const TAB_LABELS: [&str; 3] = ["One", "Two", "Three"];

/// The palette's fixed width, in logical px.
const PALETTE_W: f64 = 360.0;

/// The side pane's fixed width, in logical px.
const PANE_W: f64 = 160.0;

/// The "Shortcuts" section's state, owned by [`PlaygroundState`]. Every field
/// is a `Copy` reactive handle, so the whole struct is `Copy`.
#[derive(Clone, Copy)]
pub struct ShortcutsState {
    /// Whether the command palette is mounted.
    pub palette_open: RwSignal<bool>,
    /// The palette filter text.
    pub query: RwSignal<String>,
    /// Whether the side pane is shown.
    pub pane_open: RwSignal<bool>,
    /// The selected tab index (into [`TAB_LABELS`]).
    pub tab: RwSignal<usize>,
    /// Whether F2 has marked tab one as renamed (`One*`).
    pub renamed: RwSignal<bool>,
    /// The last consumer of a key, as a short human-readable line.
    pub last_consumer: RwSignal<String>,
    /// How many times the focusable `Button` was activated.
    pub presses: RwSignal<usize>,
    /// The focusable `Checkbox` value.
    pub checked: RwSignal<bool>,
    /// The focusable `Radio` selection.
    pub radio_on: RwSignal<bool>,
    /// The focusable `Slider` value.
    pub level: RwSignal<f64>,
    /// The focusable `TextInput` contents.
    pub field: RwSignal<String>,
}

impl ShortcutsState {
    /// Fresh state: palette and pane closed, tab one, nothing consumed yet.
    pub fn new() -> Self {
        ShortcutsState {
            palette_open: RwSignal::new(false),
            query: RwSignal::new(String::new()),
            pane_open: RwSignal::new(false),
            tab: RwSignal::new(0),
            renamed: RwSignal::new(false),
            last_consumer: RwSignal::new(String::from("none yet")),
            presses: RwSignal::new(0),
            checked: RwSignal::new(false),
            radio_on: RwSignal::new(false),
            level: RwSignal::new(0.5),
            field: RwSignal::new(String::new()),
        }
    }
}

impl Default for ShortcutsState {
    fn default() -> Self {
        Self::new()
    }
}

/// A view that hands the previous frame's shortcut hints to `make` on every
/// build and rebuild and delegates to what it returns.
///
/// `BuildCtx::shortcut_hints` is only reachable from a `View`'s own
/// `build`/`rebuild`, so this is the seam through which page code reads it.
struct WithHints<F, V> {
    make: F,
    /// The view `make` produced for the latest build/rebuild, kept so the next
    /// rebuild can diff against it (`rebuild` receives only `&prev`).
    last: RefCell<Option<Rc<V>>>,
}

fn with_hints<F, V>(make: F) -> WithHints<F, V> {
    WithHints {
        make,
        last: RefCell::new(None),
    }
}

impl<F, V> View<PlaygroundState> for WithHints<F, V>
where
    F: Fn(Rc<[ShortcutHint]>) -> V + 'static,
    V: View<PlaygroundState>,
{
    type Element = V::Element;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> Self::Element {
        let view = Rc::new((self.make)(ctx.shortcut_hints()));
        let element = view.build(ctx);
        *self.last.borrow_mut() = Some(view);
        element
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut Self::Element,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        let view = Rc::new((self.make)(ctx.shortcut_hints()));
        let prev_view = prev.last.borrow().clone();
        let flags = match prev_view {
            Some(prev_view) => view.rebuild(&prev_view, element, ctx),
            None => ChangeFlags::LAYOUT.union(ChangeFlags::PAINT),
        };
        *self.last.borrow_mut() = Some(view);
        flags
    }

    fn teardown(&self, element: &mut Self::Element, ctx: &mut BuildCtx<'_>) {
        if let Some(view) = self.last.borrow().clone() {
            view.teardown(element, ctx);
        }
    }
}

/// Live-theme accent-text role (`primary`), falling back to the Material
/// baseline pre-context.
fn accent() -> Color {
    theme().scheme().primary
}

/// A muted caption ink.
fn muted() -> Color {
    theme().scheme().on_surface_variant
}

fn theme() -> Theme {
    use_context::<Theme>().unwrap_or_else(frust_material::baseline)
}

/// Record `who` as the last consumer.
fn consumed(state: &PlaygroundState, who: impl Into<String>) {
    state.shortcuts.last_consumer.set(who.into());
}

/// The name tab `tab` shows: tab one reads `One*` while F2 has renamed it.
fn tab_name(renamed: bool, tab: usize) -> String {
    let base = TAB_LABELS[tab.min(2)];
    if tab == 0 && renamed {
        format!("{base}*")
    } else {
        base.to_string()
    }
}

/// The hints whose label or chord contains `query` (case-insensitive).
fn matching(hints: &[ShortcutHint], query: &str) -> Vec<(String, String)> {
    let query = query.to_lowercase();
    hints
        .iter()
        .filter(|hint| {
            query.is_empty()
                || hint.label.to_lowercase().contains(&query)
                || hint.chord.display().to_lowercase().contains(&query)
        })
        .map(|hint| {
            let scope = match hint.scope {
                ShortcutScope::Local => "local",
                ShortcutScope::AppGlobal => "global",
            };
            (hint.chord.display(), format!("{}  ({scope})", hint.label))
        })
        .collect()
}

/// The command palette: a filter input over the hint list.
fn palette(sc: ShortcutsState, hints: &[ShortcutHint]) -> impl View<PlaygroundState> + use<> {
    let rows = matching(hints, &sc.query.get());
    let empty = rows.is_empty();
    let mut list = column();
    for (chord, label) in rows {
        list = list.child(
            row()
                .child(SizedBox(Some(96.0), None).child(text(chord).size(12.0).color(accent())))
                .child(text(label).size(12.0)),
        );
    }
    if empty {
        list = list.child(text("No matching shortcut").size(12.0).color(muted()));
    }
    let theme = theme();
    let scheme = theme.scheme();
    container(Padding(
        EdgeInsets::all(12.0),
        column()
            .child(text("Command palette").size(13.0).color(accent()))
            .child(SizedBox(None, Some(8.0)))
            .child(
                text_input(
                    sc.query.get(),
                    move |_state: &mut PlaygroundState, value| {
                        sc.query.set(value);
                    },
                )
                .placeholder("Filter shortcuts"),
            )
            .child(SizedBox(None, Some(8.0)))
            .child(list),
    ))
    .fill(scheme.surface_container_high)
    .border(scheme.outline, 1.0)
    .radius(8.0)
    .size(PALETTE_W, 220.0)
}

/// The header the palette portal anchors to.
fn header(sc: ShortcutsState) -> impl View<PlaygroundState> + use<> {
    let key = Chord::parse("Mod+K")
        .map(|c| c.display())
        .unwrap_or_default();
    row()
        .child(text("Shortcuts").size(13.0).color(accent()))
        .child(SizedBox(Some(12.0), None))
        .child(
            button(
                format!("Palette ({key})"),
                move |state: &mut PlaygroundState| {
                    sc.palette_open.set(true);
                    consumed(state, "Button: open palette");
                },
            )
            .style(ButtonStyle::Secondary)
            .small(),
        )
}

/// The focusable-control row: the first Tab stops on the page.
fn control_row(sc: ShortcutsState) -> impl View<PlaygroundState> + use<> {
    row()
        .child(
            button(
                format!("Press ({})", sc.presses.get()),
                move |state: &mut PlaygroundState| {
                    sc.presses.update(|n| *n += 1);
                    consumed(state, "Button");
                },
            )
            .style(ButtonStyle::Primary)
            .small(),
        )
        .child(SizedBox(Some(12.0), None))
        .child(checkbox(
            sc.checked.get(),
            "Check",
            move |state: &mut PlaygroundState, value| {
                sc.checked.set(value);
                consumed(state, "Checkbox");
            },
        ))
        .child(SizedBox(Some(12.0), None))
        .child(
            radio(sc.radio_on.get(), "Radio").on_select(move |state: &mut PlaygroundState| {
                sc.radio_on.set(true);
                consumed(state, "Radio");
            }),
        )
        .child(SizedBox(Some(12.0), None))
        .child(SizedBox(Some(120.0), None).child(slider(
            sc.level.get(),
            move |state: &mut PlaygroundState, value| {
                sc.level.set(value);
                consumed(state, "Slider");
            },
        )))
        .child(SizedBox(Some(12.0), None))
        .child(
            SizedBox(Some(160.0), None).child(
                text_input(sc.field.get(), move |state: &mut PlaygroundState, value| {
                    sc.field.set(value);
                    consumed(state, "TextInput: typed text");
                })
                .placeholder("Type here"),
            ),
        )
}

/// The three tab selector buttons plus the selected tab's body.
fn tab_area(sc: ShortcutsState) -> impl View<PlaygroundState> + use<> {
    let selected = sc.tab.get();
    let renamed = sc.renamed.get();
    let tab_button = move |tab: usize| {
        button(
            format!("{} ({})", tab_name(renamed, tab), tab + 1),
            move |state: &mut PlaygroundState| {
                sc.tab.set(tab);
                consumed(state, format!("Button: tab {}", tab + 1));
            },
        )
        .style(if tab == selected {
            ButtonStyle::Primary
        } else {
            ButtonStyle::Secondary
        })
        .small()
    };
    column()
        .child(
            row()
                .child(tab_button(0))
                .child(SizedBox(Some(8.0), None))
                .child(tab_button(1))
                .child(SizedBox(Some(8.0), None))
                .child(tab_button(2)),
        )
        .child(SizedBox(None, Some(8.0)))
        .child(text(format!("Tab body: {}", tab_name(renamed, selected))).size(12.0))
}

/// The side pane shown while `pane_open` is set.
fn pane() -> impl View<PlaygroundState> + use<> {
    let theme = theme();
    let scheme = theme.scheme();
    container(Padding(
        EdgeInsets::all(12.0),
        column()
            .child(text("Side pane").size(13.0).color(accent()))
            .child(text("Toggled by Mod+B").size(11.0).color(muted())),
    ))
    .fill(scheme.surface_container)
    .radius(8.0)
    .size(PANE_W, 80.0)
}

/// The page body under the palette portal's header.
fn body(sc: ShortcutsState) -> impl View<PlaygroundState> + use<> {
    let readout = format!("Last key consumer: {}", sc.last_consumer.get());
    let main = column()
        .child(
            text(
                "Mod+K or ? opens the palette (Escape closes it); Mod+B toggles the side \
                 pane; F2 renames tab one; 1 / 2 / 3 select a tab; Tab / Shift+Tab walk \
                 the controls below.",
            )
            .size(11.0)
            .color(muted()),
        )
        .child(SizedBox(None, Some(12.0)))
        .child(control_row(sc))
        .child(SizedBox(None, Some(12.0)))
        .child(tab_area(sc))
        .child(SizedBox(None, Some(12.0)))
        .child(text(readout).size(12.0));
    row()
        .child(SizedBox(Some(560.0), None).child(main))
        .child(SizedBox(Some(12.0), None))
        .child(if sc.pane_open.get() {
            Either::Left(pane())
        } else {
            Either::Right(SizedBox::<PlaygroundState>(None, Some(0.0)))
        })
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &PlaygroundState) -> impl View<PlaygroundState> {
    let sc = state.shortcuts;
    with_hints(move |hints: Rc<[ShortcutHint]>| {
        let open = sc.palette_open.get();
        let portal = overlay_portal(header(sc))
            .overlay(open.then(|| palette(sc, &hints)))
            .placement(OverlayPlacement::on(OverlaySide::Bottom))
            .keys(if open {
                OverlayKeys::Modal
            } else {
                OverlayKeys::Pass
            })
            .on_escape(move |state: &mut PlaygroundState| {
                sc.palette_open.set(false);
                sc.query.set(String::new());
                consumed(state, "Escape: palette closed");
            });
        shortcuts(Padding(
            EdgeInsets::all(16.0),
            column()
                .child(portal)
                .child(SizedBox(None, Some(12.0)))
                .child(body(sc)),
        ))
        .global(
            "Mod+K",
            "Open palette",
            move |state: &mut PlaygroundState| {
                sc.palette_open.set(true);
                consumed(state, "Mod+K: palette opened");
            },
        )
        .global(
            "?",
            "Open palette with ?",
            move |state: &mut PlaygroundState| {
                sc.palette_open.set(true);
                consumed(state, "?: palette opened");
            },
        )
        .on(
            "Mod+B",
            "Toggle side pane",
            move |state: &mut PlaygroundState| {
                sc.pane_open.update(|open| *open = !*open);
                consumed(state, "Mod+B: pane toggled");
            },
        )
        .on("1", "Select tab one", move |state: &mut PlaygroundState| {
            sc.tab.set(0);
            consumed(state, "1: tab one");
        })
        .on("2", "Select tab two", move |state: &mut PlaygroundState| {
            sc.tab.set(1);
            consumed(state, "2: tab two");
        })
        .on(
            "3",
            "Select tab three",
            move |state: &mut PlaygroundState| {
                sc.tab.set(2);
                consumed(state, "3: tab three");
            },
        )
        .on(
            "F2",
            "Toggle tab one rename",
            move |state: &mut PlaygroundState| {
                sc.renamed.update(|on| *on = !*on);
                consumed(state, "F2: tab one rename toggled");
            },
        )
    })
}
