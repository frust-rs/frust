//! Blocks · Command: the five command-surface blocks — the ⌘K palette, the
//! morphing search, the expandable action bar, the overflow rail and the bloom
//! menu — over the sample data upstream's own previews use.
//!
//! The page is a [`component`], not a plain view function: every block here is
//! *controlled* (its open flag, its query and its selection are the caller's),
//! and [`crate::AppState`] carries no field for any of it, so the page owns its
//! own retained [`State`] behind the component boundary.
//!
//! # How the palette's keybinding is routed
//!
//! The gallery shell installs **no key handling at all**: `main.rs` builds a
//! sidebar, a page switcher and a `DesktopConfig` whose only menu item is the
//! Quit role, and `nav.rs` builds a top bar of buttons — neither reads a key,
//! and there is no accelerator to hang one on. Underneath, frust delivers
//! `InputEvent::Key` **down the recorded focus path only**, never by hit test,
//! so a widget hears a chord only while it (or a descendant) holds focus.
//!
//! There is therefore no window-level seam a page can reach for, and this page
//! supplies the binding itself, with [`key_scope`]: a transparent wrapper that
//!
//! 1. requests focus on any pointer-down landing inside it, so one click
//!    anywhere on the page arms the chord, and
//! 2. intercepts `Ctrl`/`⌘` + `K` before routing, toggling the
//!    [`CommandPaletteController`], forwarding every other key on to whatever
//!    below it holds focus — which is what keeps the palette's own ↑/↓/Enter
//!    working while it is open.
//!
//! The whole route a chord travels, once the page has been clicked: the root
//! row → `nav::shell_inset` → the page switcher → the page's `scroll_view` →
//! its `Padding` → this page's [`key_scope`] → the chord arm. Every link in
//! that chain forwards a focus-routed event to the child holding the focus
//! path (`route_event_single`; `ScrollView` spells the same rule out by hand),
//! so nothing extra has to be wired for it to arrive.
//!
//! A binding that fires *without* the page being focused would need either a
//! shell-level key seam or a `MenuItemSpec` accelerator declared in `main.rs`
//! — both outside this page.
//!
//! # Why two blocks sit in a fixed-height stage
//!
//! The palette is modal-hosted and the morphing search is anchor-hosted, and
//! `frust_beui::overlay` records the mounting contract both hosts share:
//! bounded constraints, and never inside a scroll view — a host laid out with
//! an infinite max collapses to a zero-length area on that axis. The gallery's
//! page slot *is* a scroll view, so each of those two blocks gets its own
//! fixed-height [`stage`]: a `SizedBox` tightening the height, holding a
//! `Stack` whose top child is the host. That is the "keep the host outside the
//! scroll view" remedy applied at page scale.

use std::rc::Rc;

use frust::authoring::{
    AnyView as ErasedView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, ErasedCallback,
    EventCtx, EventResult, InputEvent, Key, LayoutCtx, PaintCtx, PaintScene, Point, PointerPhase,
    Rect, SemanticsCtx, Size, View, Widget, build_child, erase_callback, rebuild_child,
    route_event_single, teardown_child, visit_children,
};
use frust::{
    AnyView, Axis, Column, Component, CrossAxisAlignment, FlexView, IconSource, SizedBox, Stack,
    TextView, any, component, icon, icons, inflexible, text,
};
use frust_beui::blocks::bloom_menu::{bloom_menu, bloom_menu_item};
use frust_beui::blocks::command_palette::{
    CommandPaletteController, CommandPaletteItem, command_palette, command_palette_item,
};
use frust_beui::blocks::expandable_action_bar::{action_bar_item, expandable_action_bar};
use frust_beui::blocks::morphing_search::{
    MorphingSearchItem, morphing_search, morphing_search_item, morphing_search_trigger,
};
use frust_beui::blocks::overflow_actions::{overflow_action, overflow_actions};
use frust_beui::components::button::{ButtonSize, ButtonTone, button};
use frust_beui::overlay::OverlayAnchor;

use crate::AppState;

// ---- The page's own state --------------------------------------------------

/// Everything the five blocks on this page are driven by.
pub struct State {
    /// The palette's open flag, shared with [`key_scope`]'s chord arm. Every
    /// dismissal — Escape, the scrim, a selection — writes `false` back into it
    /// without the page touching it.
    palette: CommandPaletteController,
    /// The palette's filter text (the field is controlled).
    palette_query: String,
    /// What the palette last reported.
    palette_log: String,
    /// The morphing search's trigger rect, published by the trigger and read by
    /// the panel.
    search_anchor: OverlayAnchor,
    search_open: bool,
    /// Whether the trigger draws its compact icon square instead of the field.
    search_icon_only: bool,
    search_query: String,
    search_log: String,
    /// The action bar's expansion, taken over so the demo's own toggle works
    /// alongside hover and focus.
    bar_expanded: bool,
    bar_active: usize,
    bar_log: String,
    /// The overflow rail's expansion and the width its container is pinned to —
    /// the rail splits on available width, so the width is the demo.
    overflow_expanded: bool,
    rail_width: f64,
    overflow_log: String,
    bloom_open: bool,
    bloom_log: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            palette: CommandPaletteController::new(),
            palette_query: String::new(),
            palette_log: "(nothing run yet)".to_string(),
            search_anchor: OverlayAnchor::new(),
            search_open: false,
            search_icon_only: false,
            search_query: String::new(),
            search_log: "(nothing opened yet)".to_string(),
            bar_expanded: false,
            bar_active: 0,
            bar_log: "(no action yet)".to_string(),
            overflow_expanded: false,
            rail_width: RAIL_WIDTHS[1].1,
            overflow_log: "(no action yet)".to_string(),
            bloom_open: false,
            bloom_log: "(nothing created yet)".to_string(),
        }
    }
}

/// The three container widths the overflow rail is demonstrated at.
const RAIL_WIDTHS: [(&str, f64); 3] = [("Narrow", 220.0), ("Medium", 340.0), ("Wide", 520.0)];

// ---- Page chrome -----------------------------------------------------------

/// The page's own title.
fn heading(title: &str) -> TextView {
    text(title.to_string()).size(24.0)
}

/// A block's title.
fn section(title: &str) -> TextView {
    text(title.to_string()).size(16.0)
}

/// A block's small print — what it is, and what the port decided.
fn caption(body: impl Into<String>) -> TextView {
    text(body.into()).size(12.0)
}

/// A vertical gap.
fn gap(height: f64) -> AnyView<State> {
    any(SizedBox(None, Some(height)))
}

/// A horizontal gap.
fn hgap(width: f64) -> AnyView<State> {
    any(SizedBox(Some(width), None))
}

/// A centred row of controls.
fn controls(children: Vec<AnyView<State>>) -> AnyView<State> {
    let mut spaced = Vec::with_capacity(children.len() * 2);
    for (index, child) in children.into_iter().enumerate() {
        if index > 0 {
            spaced.push(hgap(8.0));
        }
        spaced.push(child);
    }
    any(FlexView::new(
        Axis::Horizontal,
        spaced.into_iter().map(inflexible).collect(),
    )
    .cross_axis(CrossAxisAlignment::Center))
}

/// A fixed-height mounting box for an overlay host: the bounded constraints
/// `frust_beui::overlay` requires, and a `Stack` to put the host on top of the
/// block's own explanatory content. See the [module docs](self).
fn stage(height: f64, children: Vec<AnyView<State>>) -> AnyView<State> {
    any(SizedBox(None, Some(height)).child(Stack(children)))
}

// ---- The command palette ---------------------------------------------------

/// The palette's commands, upstream's own preview set.
fn palette_items() -> Vec<CommandPaletteItem> {
    vec![
        command_palette_item("Go to Home")
            .group("Navigation")
            .hint("G H")
            .keywords(["start", "index"]),
        command_palette_item("Open profile")
            .group("Navigation")
            .hint("G P")
            .keywords(["account", "user"]),
        command_palette_item("Settings")
            .group("Navigation")
            .keywords(["preferences", "config"]),
        command_palette_item("Create document")
            .group("Actions")
            .hint("\u{2318} N")
            .keywords(["new", "file"]),
        command_palette_item("New project")
            .group("Actions")
            .hint("\u{2318} \u{21e7} N")
            .keywords(["new", "workspace"]),
    ]
}

/// The palette block: an open button, the chord's own readout, and the modal
/// host itself over a fixed-height stage.
fn palette_block(state: &State) -> AnyView<State> {
    let items = palette_items();
    let labels: Vec<String> = items.iter().map(|item| item.label().to_string()).collect();
    let query = state.palette_query.clone();

    // The stage is 560px so the panel's own `60vh` cap clears the five rows and
    // their two group headings rather than clipping them.
    let mounting_note = any(caption("The palette mounts over this box."));

    any(Column(vec![
        any(section("command_palette")),
        gap(6.0),
        any(caption(
            "Grouped commands behind a fuzzy filter. The panel intercepts \u{2191}/\u{2193}/Enter \
             before the field sees them; Escape is left to the modal host so it can play the \
             exit. The global shortcut is the app's binding, not the block's — this page's own \
             key scope supplies it. There is no focus-on-appear hook in the framework, so the \
             panel takes focus on the first press that lands on it; that press is what makes its \
             own arrows and Enter reachable.",
        )),
        gap(10.0),
        controls(vec![
            any(button("Open command palette", |s: &mut State| {
                s.palette.open();
            })),
            any(caption(
                "\u{2026} or press Ctrl / \u{2318} + K after clicking the page",
            )),
        ]),
        gap(8.0),
        any(caption(format!("Last command: {}", state.palette_log))),
        gap(10.0),
        stage(
            560.0,
            vec![
                mounting_note,
                any(command_palette(
                    items,
                    query,
                    |s: &mut State, text: String| s.palette_query = text,
                    move |s: &mut State, index: usize| {
                        s.palette_log = labels
                            .get(index)
                            .cloned()
                            .unwrap_or_else(|| format!("index {index}"));
                        s.palette_query.clear();
                    },
                )
                .controller(&state.palette)
                .label("Gallery command palette")
                .on_open_change(|s: &mut State, open: bool| {
                    // The controller already holds the flag; mirroring the edge is
                    // what makes the page rebuild around it.
                    s.palette.set_open(open);
                    if !open {
                        s.palette_query.clear();
                    }
                })),
            ],
        ),
    ]))
}

// ---- The morphing search ---------------------------------------------------

/// The search's entries, upstream's own preview set.
fn search_items() -> Vec<MorphingSearchItem> {
    vec![
        morphing_search_item("Project Folder")
            .description("Block \u{b7} Files and previews")
            .keywords(["files", "overlay"]),
        morphing_search_item("Motion components")
            .description("Collection \u{b7} Interaction primitives")
            .keywords(["animation", "components"]),
        morphing_search_item("Agent interfaces")
            .description("Collection \u{b7} AI building blocks")
            .keywords(["ai", "chat"]),
        morphing_search_item("Installation guide")
            .description("Documentation \u{b7} Add your first component")
            .keywords(["setup", "shadcn"]),
        morphing_search_item("Design tokens")
            .description("Documentation \u{b7} Color, type, and motion")
            .keywords(["theme", "styles"]),
    ]
}

/// The morphing-search block: the closed trigger, a shape toggle, and the panel
/// that morphs out of the trigger's own box.
fn search_block(state: &State) -> AnyView<State> {
    let items = search_items();
    let titles: Vec<String> = items.iter().map(|item| item.title().to_string()).collect();
    let query = state.search_query.clone();

    // The trigger sits at the stage's own top edge: the panel's top lands on
    // the trigger's, so anything above it inside the stage would be painted
    // over. The block's prose stays outside the stage for the same reason.
    let body = any(Column(vec![controls(vec![
        any(morphing_search_trigger::<State>(&state.search_anchor)
            .placeholder("Find components")
            .shortcut(Some("F"))
            .icon_only(state.search_icon_only)
            .open(state.search_open)
            .on_open_change(|s: &mut State, open: bool| s.search_open = open)),
        any(button(
            if state.search_icon_only {
                "Full field"
            } else {
                "Icon only"
            },
            |s: &mut State| s.search_icon_only = !s.search_icon_only,
        )
        .tone(ButtonTone::Secondary)
        .size(ButtonSize::Sm)),
    ])]));

    any(Column(vec![
        any(section("morphing_search")),
        gap(6.0),
        any(caption(
            "A field-shaped trigger that morphs into its own results panel, anchored to the rect \
             the trigger publishes. The shape toggle below switches the same trigger between the \
             full field and the compact icon square.",
        )),
        gap(10.0),
        stage(
            400.0,
            vec![
                body,
                any(morphing_search(
                    items,
                    query,
                    |s: &mut State, text: String| s.search_query = text,
                    move |s: &mut State, index: usize| {
                        s.search_log = titles
                            .get(index)
                            .cloned()
                            .unwrap_or_else(|| format!("index {index}"));
                        s.search_query.clear();
                    },
                )
                .anchor(&state.search_anchor)
                .open(state.search_open)
                .placeholder("Find components")
                .on_open_change(|s: &mut State, open: bool| {
                    s.search_open = open;
                    if !open {
                        s.search_query.clear();
                    }
                })),
            ],
        ),
        gap(8.0),
        any(caption(format!("Last opened: {}", state.search_log))),
    ]))
}

// ---- The expandable action bar ---------------------------------------------

/// The action bar block: upstream's six actions, plus the controlled-expansion
/// toggle its own preview ships.
fn action_bar_block(state: &State) -> AnyView<State> {
    let entries: [(&str, IconSource, Option<&str>); 6] = [
        ("Send", icons::SEND, Some("S")),
        ("Copy", icons::CONTENT_COPY, Some("C")),
        ("Export", icons::ARROW_DOWNWARD, Some("E")),
        ("Archive", icons::ARCHIVE, None),
        ("Alerts", icons::NOTIFICATIONS, None),
        ("Settings", icons::SETTINGS, None),
    ];
    let labels: Vec<String> = entries
        .iter()
        .map(|(label, _, _)| label.to_string())
        .collect();
    let active = state.bar_active;
    let items = entries
        .iter()
        .enumerate()
        .map(|(index, (label, glyph, shortcut))| {
            let item = action_bar_item(icon(*glyph).size(16.0), *label).active(index == active);
            match shortcut {
                Some(key) => item.shortcut(*key),
                None => item,
            }
        })
        .collect();

    any(Column(vec![
        any(section("expandable_action_bar")),
        gap(6.0),
        any(caption(
            "Icon-only until hovered or focused, when every item springs open to its label and \
             shortcut. The expansion is taken over here so the toggle below drives it directly.",
        )),
        gap(10.0),
        any(
            expandable_action_bar(items, move |s: &mut State, index: usize| {
                s.bar_active = index;
                s.bar_log = labels
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| format!("index {index}"));
            })
            .expanded(state.bar_expanded)
            .on_expanded_change(|s: &mut State, expanded: bool| s.bar_expanded = expanded),
        ),
        gap(10.0),
        controls(vec![
            any(button(
                if state.bar_expanded {
                    "Collapse"
                } else {
                    "Expand"
                },
                |s: &mut State| s.bar_expanded = !s.bar_expanded,
            )
            .tone(ButtonTone::Secondary)
            .size(ButtonSize::Sm)),
            any(caption(format!("Last action: {}", state.bar_log))),
        ]),
    ]))
}

// ---- The overflow rail -----------------------------------------------------

/// The overflow block: the rail inside a container whose width the demo picks,
/// which is what makes the collapse visible.
fn overflow_block(state: &State) -> AnyView<State> {
    let primary = vec![
        overflow_action("Preview"),
        overflow_action("Pin"),
        overflow_action("Rename"),
    ];
    let overflow = vec![
        overflow_action("Branch"),
        overflow_action("Schedule"),
        overflow_action("Delete").disabled(true),
    ];
    let labels: Vec<String> = primary
        .iter()
        .chain(overflow.iter())
        .map(|item| item.label().to_string())
        .collect();

    let width_buttons: Vec<AnyView<State>> = RAIL_WIDTHS
        .iter()
        .map(|(label, width)| {
            let width = *width;
            any(button(*label, move |s: &mut State| s.rail_width = width)
                .tone(if (state.rail_width - width).abs() < 0.5 {
                    ButtonTone::Primary
                } else {
                    ButtonTone::Outline
                })
                .size(ButtonSize::Sm))
        })
        .collect();

    any(Column(vec![
        any(section("overflow_actions")),
        gap(6.0),
        any(caption(
            "The rail keeps as many primary actions on screen as the width allows and files the \
             rest behind the toggle. `on_action` addresses one concatenated list, so an action \
             pushed into the overflow group keeps its own index — the split is layout, never a \
             renumbering.",
        )),
        gap(10.0),
        controls(width_buttons),
        gap(10.0),
        any(SizedBox(Some(state.rail_width), None).child(
            overflow_actions(primary, overflow, move |s: &mut State, index: usize| {
                s.overflow_log = labels
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| format!("index {index}"));
            })
            .expanded(state.overflow_expanded)
            .collapse_on_action(true)
            .on_expanded_change(|s: &mut State, expanded: bool| s.overflow_expanded = expanded),
        )),
        gap(8.0),
        any(caption(format!(
            "Container {:.0}px \u{b7} last action: {}",
            state.rail_width, state.overflow_log
        ))),
    ]))
}

// ---- The bloom menu --------------------------------------------------------

/// The bloom block: a labelled trigger that irises open into a three-column
/// grid blooming out of its own centre.
fn bloom_block(state: &State) -> AnyView<State> {
    let entries: [(&str, IconSource); 6] = [
        ("Document", icons::DESCRIPTION),
        ("Image", icons::IMAGE),
        ("Link", icons::LINK),
        ("Meeting", icons::SCHEDULE),
        ("Task", icons::DONE_ALL),
        ("Team", icons::GROUP),
    ];
    let labels: Vec<String> = entries.iter().map(|(label, _)| label.to_string()).collect();
    let items = entries
        .iter()
        .map(|(label, glyph)| bloom_menu_item(icon(*glyph).size(20.0), *label))
        .collect();

    any(Column(vec![
        any(section("bloom_menu")),
        gap(6.0),
        any(caption(
            "The trigger morphs into a panel and the grid blooms out of its centre, each cell \
             delayed by its radial distance. The block reserves its own centring box, so opening \
             it never reflows the page.",
        )),
        gap(10.0),
        any(bloom_menu(items, move |s: &mut State, index: usize| {
            s.bloom_log = labels
                .get(index)
                .cloned()
                .unwrap_or_else(|| format!("index {index}"));
        })
        .open(state.bloom_open)
        .trigger_label("Create")
        .heading("Create")
        .on_open_change(|s: &mut State, open: bool| s.bloom_open = open)),
        gap(8.0),
        any(caption(format!("Last created: {}", state.bloom_log))),
    ]))
}

// ---- The page --------------------------------------------------------------

/// The Blocks · Command page.
#[derive(Default)]
struct CommandPage;

impl Component for CommandPage {
    type State = State;

    fn init(&self) -> State {
        State::default()
    }

    fn build(&self, state: &mut State) -> impl View<State> {
        any(key_scope(
            Column(vec![
                any(heading("Blocks \u{b7} Command")),
                gap(8.0),
                any(caption(
                    "Five command surfaces. Click anywhere on this page once to arm the \
                     Ctrl / \u{2318} + K binding — see this module's docs for the route it takes.",
                )),
                gap(24.0),
                palette_block(state),
                gap(24.0),
                search_block(state),
                gap(24.0),
                action_bar_block(state),
                gap(24.0),
                overflow_block(state),
                gap(24.0),
                bloom_block(state),
            ]),
            |s: &mut State| {
                s.palette.toggle();
            },
        ))
    }
}

/// The Blocks · Command page, hosted over its own retained [`State`].
pub fn page() -> AnyView<AppState> {
    any(component(CommandPage))
}

// ---- The page's key scope --------------------------------------------------

/// Wrap `child` in a focus scope that turns `Ctrl`/`⌘` + `K` into `on_chord`.
///
/// See the [module docs](self) for why the binding lives here rather than in
/// the shell, and for the route a chord actually travels.
fn key_scope<State: 'static, V: View<State>>(
    child: V,
    on_chord: impl Fn(&mut State) + 'static,
) -> KeyScopeView<State> {
    KeyScopeView {
        child: any(child),
        on_chord: Rc::new(on_chord),
    }
}

/// A declarative key scope. See [`key_scope`].
struct KeyScopeView<State: 'static> {
    child: ErasedView<State>,
    on_chord: Rc<dyn Fn(&mut State)>,
}

/// The retained widget for a [`KeyScopeView`].
struct KeyScopeWidget {
    child: ChildPod,
    on_chord: ErasedCallback,
}

impl<State: 'static> View<State> for KeyScopeView<State> {
    type Element = KeyScopeWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> KeyScopeWidget {
        KeyScopeWidget {
            child: build_child(&self.child, ctx),
            on_chord: erase_callback(&self.on_chord),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut KeyScopeWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // Closures aren't comparable, so the adapter is reinstalled
        // unconditionally — what every interactive widget in the catalog does.
        element.on_chord = erase_callback(&self.on_chord);
        rebuild_child(&prev.child, &self.child, &mut element.child, ctx)
    }

    fn teardown(&self, element: &mut KeyScopeWidget, ctx: &mut BuildCtx<'_>) {
        teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for KeyScopeWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ORIGIN);
        bc.constrain(size)
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        // The chord is claimed *before* routing: it must win over whatever
        // below holds focus (the palette's own field, most of the time).
        if let InputEvent::Key(key) = event
            && !key.repeat
            && (key.modifiers.ctrl || key.modifiers.meta)
            && matches!(&key.key, Key::Character(c) if c.eq_ignore_ascii_case("k"))
        {
            (self.on_chord)(ctx);
            ctx.request_redraw();
            return EventResult::Handled;
        }

        let result = route_event_single(&mut self.child, ctx, event);

        // Focus-arming: a press anywhere inside the scope puts it on the focus
        // path, which is the only way a later key reaches this widget at all.
        // Claimed after routing, so a child that wants focus for itself is
        // recorded first (the container-claims-after-routing rule).
        if let InputEvent::Pointer(pointer) = event
            && pointer.phase == PointerPhase::Down
            && Rect::from_origin_size(Point::ORIGIN, ctx.size()).contains(pointer.position)
        {
            ctx.request_focus();
        }
        result
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        self.child.semantics_child(ctx);
    }

    visit_children!(child);
}
