//! Controls: interactive toggle/selection widgets — checkbox, switch, radio
//! group, toggle, toggle group, slider, accordion, collapsible, tabs,
//! pagination. Every widget here is controlled: this page's `State` holds
//! the current value and every callback writes it back through the full
//! `AppState` path.

use frust::{SizedBox, View, any, column, row, text};
use frust_shadcn::{
    AccordionMode, PaginationItem, ToggleGroupMode, ToggleVariant, accordion, accordion_item,
    checkbox, collapsible, label, pagination, radio_group, radio_group_item, separator, slider,
    switch, tabs, tabs_tab, toggle, toggle_group, toggle_group_item,
};

use crate::AppState;

pub struct State {
    pub checkbox_checked: bool,
    pub switch_checked: bool,
    pub radio_value: String,
    pub toggle_pressed: bool,
    pub toggle_group_selected: Vec<String>,
    pub slider_value: Vec<f64>,
    pub accordion_open: Vec<usize>,
    pub collapsible_open: bool,
    pub tabs_value: String,
    pub pagination_current: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            checkbox_checked: true,
            switch_checked: false,
            radio_value: "comfortable".to_string(),
            toggle_pressed: false,
            toggle_group_selected: vec!["bold".to_string()],
            slider_value: vec![40.0],
            accordion_open: vec![0],
            collapsible_open: false,
            tabs_value: "account".to_string(),
            pagination_current: 2,
        }
    }
}

fn gap() -> frust::AnyView<AppState> {
    any(SizedBox(None, Some(12.0)))
}

pub fn page(state: &mut State) -> impl View<AppState> + use<> {
    let checkbox_checked = state.checkbox_checked;
    let switch_checked = state.switch_checked;
    let radio_value = state.radio_value.clone();
    let toggle_pressed = state.toggle_pressed;
    let toggle_group_selected = state.toggle_group_selected.clone();
    let slider_value = state.slider_value.clone();
    let accordion_open = state.accordion_open.clone();
    let collapsible_open = state.collapsible_open;
    let tabs_value = state.tabs_value.clone();
    let pagination_current = state.pagination_current;

    column()
        .child(crate::nav::heading("Controls"))
        .child(SizedBox(None, Some(16.0)))
        .child(
            row()
                .child(
                    checkbox(checkbox_checked, |s: &mut AppState, v: bool| {
                        s.controls.checkbox_checked = v;
                    })
                    .label("Accept terms"),
                )
                .child(SizedBox(Some(24.0), None))
                .child(
                    checkbox(false, |_: &mut AppState, _: bool| {})
                        .label("Disabled")
                        .disabled(true),
                )
                .child(SizedBox(Some(24.0), None))
                .child(
                    switch(switch_checked, |s: &mut AppState, v: bool| {
                        s.controls.switch_checked = v;
                    })
                    .label("Airplane mode"),
                ),
        )
        .child(gap())
        .child(
            row()
                .child(toggle(
                    "Bold",
                    toggle_pressed,
                    |s: &mut AppState, v: bool| {
                        s.controls.toggle_pressed = v;
                    },
                ))
                .child(SizedBox(Some(16.0), None))
                .child(
                    toggle_group(
                        vec![
                            toggle_group_item("bold", "B"),
                            toggle_group_item("italic", "I"),
                            toggle_group_item("underline", "U").disabled(true),
                        ],
                        toggle_group_selected,
                        |s: &mut AppState, v: Vec<String>| {
                            s.controls.toggle_group_selected = v;
                        },
                    )
                    .mode(ToggleGroupMode::Multiple)
                    .variant(ToggleVariant::Outline),
                ),
        )
        .child(gap())
        .child(radio_group(
            radio_value,
            vec![
                radio_group_item("default").label("Default"),
                radio_group_item("comfortable").label("Comfortable"),
                radio_group_item("compact").label("Compact"),
            ],
            |s: &mut AppState, v: String| {
                s.controls.radio_value = v;
            },
        ))
        .child(gap())
        .child(
            slider(slider_value, |s: &mut AppState, v: Vec<f64>| {
                s.controls.slider_value = v;
            })
            .label("Volume"),
        )
        .child(gap())
        .child(separator())
        .child(gap())
        .child(
            accordion(
                vec![
                    accordion_item(
                        "Is it accessible?",
                        text("Yes, it follows the WAI-ARIA pattern."),
                    ),
                    accordion_item("Is it styled?", text("Yes, it uses shadcn's own tokens.")),
                    accordion_item("Disabled item", text("Never shown while disabled."))
                        .disabled(true),
                ],
                accordion_open,
                |s: &mut AppState, open: Vec<usize>| {
                    s.controls.accordion_open = open;
                },
            )
            .mode(AccordionMode::Single),
        )
        .child(gap())
        .child(collapsible(
            label(if collapsible_open {
                "Hide details"
            } else {
                "Show details"
            }),
            text("Three more items are revealed here."),
            collapsible_open,
            |s: &mut AppState, v: bool| {
                s.controls.collapsible_open = v;
            },
        ))
        .child(gap())
        .child(tabs(
            tabs_value,
            vec![
                tabs_tab("account", "Account", text("Account settings go here.")),
                tabs_tab("password", "Password", text("Password settings go here.")),
            ],
            |s: &mut AppState, v: String| {
                s.controls.tabs_value = v;
            },
        ))
        .child(gap())
        .child(pagination(
            vec![
                PaginationItem::Page(1),
                PaginationItem::Page(2),
                PaginationItem::Page(3),
                PaginationItem::Ellipsis,
                PaginationItem::Page(10),
            ],
            pagination_current,
            |s: &mut AppState, page: usize| {
                s.controls.pagination_current = page;
            },
        ))
}
