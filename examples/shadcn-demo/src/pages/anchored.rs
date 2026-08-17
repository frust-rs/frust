//! Anchored: the non-modal, trigger-relative overlay family — popover,
//! tooltip, hover-card, dropdown menu, context menu, select and combobox.
//! Every click-driven one mounts as the top child of a full-area
//! [`frust::Stack`] (this page's own content IS that stack — see
//! `frust_shadcn::overlay`'s module docs for the two supported mounts), gated
//! on an `open` flag this page's `State` owns; the two hover-driven ones
//! (tooltip, hover-card) mount **permanently**, input-transparent, per their
//! own module docs.

use frust::{Column, Row, SizedBox, Stack, View, any, text};
use frust_shadcn::overlay::{OverlayAlign, OverlayAnchor, OverlaySide, anchor};
use frust_shadcn::{
    ButtonVariant, TooltipHover, combobox, combobox_item, combobox_trigger, context_menu,
    context_menu_trigger, dropdown_menu, dropdown_menu_item, dropdown_menu_label,
    dropdown_menu_separator, hover_card, hover_card_trigger, popover, select, select_option,
    select_trigger, tooltip, tooltip_trigger,
};

use crate::AppState;

const SIDES: [OverlaySide; 4] = [
    OverlaySide::Top,
    OverlaySide::Right,
    OverlaySide::Bottom,
    OverlaySide::Left,
];
const ALIGNS: [OverlayAlign; 3] = [OverlayAlign::Start, OverlayAlign::Center, OverlayAlign::End];
const OFFSETS: [f64; 4] = [0.0, 4.0, 12.0, 24.0];

pub struct State {
    pub popover_anchor: OverlayAnchor,
    pub popover_open: bool,
    pub popover_side: usize,
    pub popover_align: usize,
    pub popover_offset: usize,

    pub flip_anchor: OverlayAnchor,
    pub flip_open: bool,

    pub tooltip_hover: TooltipHover,
    pub hover_card_hover: TooltipHover,

    pub dropdown_anchor: OverlayAnchor,
    pub dropdown_open: bool,
    pub dropdown_bold: bool,
    pub dropdown_align: String,

    pub context_menu_anchor: OverlayAnchor,
    pub context_menu_open: bool,

    pub select_anchor: OverlayAnchor,
    pub select_open: bool,
    pub select_value: Option<usize>,

    pub combobox_anchor: OverlayAnchor,
    pub combobox_open: bool,
    pub combobox_query: String,
    pub combobox_value: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            popover_anchor: OverlayAnchor::new(),
            popover_open: false,
            popover_side: 2,
            popover_align: 1,
            popover_offset: 1,

            flip_anchor: OverlayAnchor::new(),
            flip_open: false,

            tooltip_hover: TooltipHover::new(),
            hover_card_hover: TooltipHover::new(),

            dropdown_anchor: OverlayAnchor::new(),
            dropdown_open: false,
            dropdown_bold: true,
            dropdown_align: "left".to_string(),

            context_menu_anchor: OverlayAnchor::new(),
            context_menu_open: false,

            select_anchor: OverlayAnchor::new(),
            select_open: false,
            select_value: Some(2),

            combobox_anchor: OverlayAnchor::new(),
            combobox_open: false,
            combobox_query: String::new(),
            combobox_value: None,
        }
    }
}

const FRUITS: [&str; 6] = ["Apple", "Banana", "Cherry", "Date", "Elderberry", "Fig"];

fn gap() -> frust::AnyView<AppState> {
    any(SizedBox(None, Some(12.0)))
}

pub fn page(state: &mut State) -> impl View<AppState> + use<> {
    let popover_anchor = state.popover_anchor.clone();
    let popover_open = state.popover_open;
    let side = SIDES[state.popover_side % SIDES.len()];
    let align = ALIGNS[state.popover_align % ALIGNS.len()];
    let offset = OFFSETS[state.popover_offset % OFFSETS.len()];

    let flip_anchor = state.flip_anchor.clone();
    let flip_open = state.flip_open;

    let tooltip_hover = state.tooltip_hover.clone();
    let hover_card_hover = state.hover_card_hover.clone();

    let dropdown_anchor = state.dropdown_anchor.clone();
    let dropdown_open = state.dropdown_open;
    let dropdown_bold = state.dropdown_bold;
    let dropdown_align = state.dropdown_align.clone();

    let context_menu_anchor = state.context_menu_anchor.clone();
    let context_menu_open = state.context_menu_open;

    let select_anchor = state.select_anchor.clone();
    let select_open = state.select_open;
    let select_value = state.select_value;

    let combobox_anchor = state.combobox_anchor.clone();
    let combobox_open = state.combobox_open;
    let combobox_query = state.combobox_query.clone();
    let combobox_value = state.combobox_value.clone();

    let base = Column(vec![
        any(crate::nav::heading("Anchored")),
        any(SizedBox(None, Some(16.0))),
        any(text(
            "Overlays here need one pointer press inside them before Escape \
             or an arrow key does anything — no auto-focus-on-appear hook \
             exists in the framework yet.",
        )
        .size(12.0)),
        gap(),
        // --- Popover: placement knobs + light-dismiss + Escape-after-press ---
        any(Row(vec![
            any(anchor(
                &popover_anchor,
                button("Toggle Popover", move |s: &mut AppState| {
                    s.anchored.popover_open = !s.anchored.popover_open;
                }),
            )),
            any(SizedBox(Some(16.0), None)),
            any(button("side \u{2192}", move |s: &mut AppState| {
                s.anchored.popover_side = s.anchored.popover_side.wrapping_add(1);
            })
            .variant(ButtonVariant::Outline)),
            any(SizedBox(Some(8.0), None)),
            any(button("align \u{2192}", move |s: &mut AppState| {
                s.anchored.popover_align = s.anchored.popover_align.wrapping_add(1);
            })
            .variant(ButtonVariant::Outline)),
            any(SizedBox(Some(8.0), None)),
            any(button("offset \u{2192}", move |s: &mut AppState| {
                s.anchored.popover_offset = s.anchored.popover_offset.wrapping_add(1);
            })
            .variant(ButtonVariant::Outline)),
        ])),
        gap(),
        // --- Tooltip: hover-delay + click-through proof ---
        any(anchor(
            &OverlayAnchor::new(),
            tooltip_trigger(
                &tooltip_hover,
                button("Hover me (tooltip, ~700ms)", |_: &mut AppState| {}),
            ),
        )),
        gap(),
        // --- Hover card: 300ms grace crossing trigger -> panel ---
        any(hover_card_trigger(
            &hover_card_hover,
            button("Hover me (hover card)", |_: &mut AppState| {}),
        )),
        gap(),
        // --- Dropdown menu: full row vocabulary ---
        any(anchor(
            &dropdown_anchor,
            button("Open Dropdown Menu", move |s: &mut AppState| {
                s.anchored.dropdown_open = !s.anchored.dropdown_open;
            }),
        )),
        gap(),
        // --- Context menu: left-click-only shell caveat ---
        any(text(
            "Context menu area below — right-click won't reach it live: \
             the desktop shell forwards only the left mouse button today \
             (frust-shell-desktop's app_handler.rs). The component itself \
             is complete and driven in its own tests.",
        )
        .size(12.0)),
        any(SizedBox(None, Some(4.0))),
        any(context_menu_trigger(
            &context_menu_anchor,
            frust::SizedBox::<AppState>(Some(240.0), Some(60.0))
                .child(text("Right-click here (see caveat above)").size(13.0)),
        )),
        gap(),
        // --- Select: grouped, disabled option, long scrolling list ---
        any(anchor(
            &select_anchor,
            select_trigger(&select_anchor, fruit_options(), select_value)
                .placeholder("Pick a fruit")
                .on_open_change(move |s: &mut AppState, open: bool| {
                    s.anchored.select_open = open;
                }),
        )),
        gap(),
        // --- Combobox: type-to-filter + empty state + commit updates trigger ---
        any(anchor(
            &combobox_anchor,
            combobox_trigger::<AppState>(&combobox_anchor, combobox_value.clone())
                .placeholder("Search fruit…")
                .on_open_change(move |s: &mut AppState, open: bool| {
                    s.anchored.combobox_open = open;
                }),
        )),
        any(SizedBox(None, Some(200.0))),
        any(text(
            "The trigger below is parked near the scroll view's bottom edge \
             so its popover has no room to open downward — watch it flip to \
             open above instead (avoidCollisions' default flip).",
        )
        .size(12.0)),
        any(SizedBox(None, Some(8.0))),
        any(anchor(
            &flip_anchor,
            button("Open near the bottom edge", move |s: &mut AppState| {
                s.anchored.flip_open = !s.anchored.flip_open;
            }),
        )),
        any(SizedBox(None, Some(24.0))),
    ]);

    let mut layers: Vec<frust::AnyView<AppState>> = vec![any(base)];

    if popover_open {
        layers.push(any(popover(text(
            "Placement knobs move me around the trigger.",
        ))
        .anchor(&popover_anchor)
        .side(side)
        .align(align)
        .offset(offset)
        .on_open_change(|s: &mut AppState, open: bool| {
            s.anchored.popover_open = open;
        })));
    }

    if flip_open {
        layers.push(any(popover(text("I flip when the bottom edge is tight."))
            .anchor(&flip_anchor)
            .on_open_change(|s: &mut AppState, open: bool| {
                s.anchored.flip_open = open;
            })));
    }

    layers.push(any(tooltip(&tooltip_hover, "A tooltip, 700ms after rest")));
    layers.push(any(hover_card(
        &hover_card_hover,
        Column(vec![
            any(text("Hover card").size(14.0)),
            any(text("Crossing onto this panel keeps it open.").size(12.0)),
        ]),
    )));

    if dropdown_open {
        layers.push(any(dropdown_menu(
            vec![
                dropdown_menu_label("Actions"),
                dropdown_menu_item("Bold")
                    .checked(dropdown_bold)
                    .shortcut("\u{2318}B"),
                dropdown_menu_item("Italic").shortcut("\u{2318}I"),
                dropdown_menu_separator(),
                dropdown_menu_item("Align left").radio(dropdown_align == "left"),
                dropdown_menu_item("Align center").radio(dropdown_align == "center"),
                dropdown_menu_item("Align right").radio(dropdown_align == "right"),
                dropdown_menu_separator(),
                dropdown_menu_item("Disabled row").disabled(true),
                dropdown_menu_item("More").submenu(vec![
                    dropdown_menu_item("Duplicate"),
                    dropdown_menu_item("Archive"),
                ]),
            ],
            move |s: &mut AppState, index: usize| match index {
                1 => s.anchored.dropdown_bold = !s.anchored.dropdown_bold,
                4 => s.anchored.dropdown_align = "left".to_string(),
                5 => s.anchored.dropdown_align = "center".to_string(),
                6 => s.anchored.dropdown_align = "right".to_string(),
                _ => {}
            },
        )
        .anchor(&dropdown_anchor)
        .on_open_change(|s: &mut AppState, open: bool| {
            s.anchored.dropdown_open = open;
        })));
    }

    if context_menu_open {
        layers.push(any(context_menu(
            vec![
                dropdown_menu_item("Copy").shortcut("\u{2318}C"),
                dropdown_menu_item("Paste").shortcut("\u{2318}V"),
                dropdown_menu_separator(),
                dropdown_menu_item("Delete"),
            ],
            |_: &mut AppState, _index: usize| {},
        )
        .anchor(&context_menu_anchor)
        .on_open_change(|s: &mut AppState, open: bool| {
            s.anchored.context_menu_open = open;
        })));
    }

    if select_open {
        layers.push(any(select(
            fruit_options(),
            select_value,
            |s: &mut AppState, index: usize| {
                s.anchored.select_value = Some(index);
                s.anchored.select_open = false;
            },
        )
        .anchor(&select_anchor)));
    }

    if combobox_open {
        layers.push(any(combobox(
            FRUITS.iter().map(|f| combobox_item(*f)).collect(),
            combobox_query,
            |s: &mut AppState, q: String| {
                s.anchored.combobox_query = q;
            },
            move |s: &mut AppState, index: usize| {
                s.anchored.combobox_value = FRUITS.get(index).map(|f| f.to_string());
                s.anchored.combobox_open = false;
                s.anchored.combobox_query.clear();
            },
        )
        .anchor(&combobox_anchor)));
    }

    Stack(layers)
}

fn button<F: Fn(&mut AppState) + 'static>(
    label: impl Into<String>,
    on_press: F,
) -> frust_shadcn::ButtonView<AppState> {
    frust_shadcn::button(label, on_press)
}

fn fruit_options() -> Vec<frust_shadcn::SelectOption> {
    let mut options = Vec::new();
    for (i, f) in FRUITS.iter().enumerate() {
        let mut opt = select_option(*f).group(if i < 3 { "Common" } else { "Rare" });
        if *f == "Date" {
            opt = opt.disabled(true);
        }
        options.push(opt);
    }
    for extra in ["Guava", "Honeydew", "Kiwi", "Lychee", "Mango", "Nectarine"] {
        options.push(select_option(extra).group("Rare"));
    }
    options
}
