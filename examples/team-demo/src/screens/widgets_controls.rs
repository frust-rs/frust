//! Widgets · Controls screen — REAL (ported from `examples/catalog` in task 03).
//!
//! The catalog's Controls tab, exhibit-for-exhibit: a [`Switch`]/
//! [`cupertino_switch`], assist/filter [chips](assist_chip), a
//! [`fab`]/[`extended_fab`], determinate/indeterminate linear + circular
//! [`ProgressValue`] indicators (flat and [`.wavy()`](forgekit::LinearProgressView::wavy)),
//! a [`loading_indicator`], a [`button_group`], a [`split_button`], floating +
//! docked [toolbars](floating_toolbar), a bottom-right [`fab_menu`] overlay, a
//! [`slider`], and — in Cupertino mode — glass-navbar/tabbar notes plus capsule
//! [`cupertino_button`]s at every size class.
//!
//! # Shape
//!
//! `routes.rs` (a finalized shared file) calls this screen with **no**
//! arguments, and a router page builder is re-invoked every rebuild (see
//! `forgekit_widgets::navigator`'s module docs), so the exhibit's mutable state
//! can't be a plain local (it would reset each frame). The screen is therefore a
//! nested [`Component`] whose [retained state](ControlsState) bundles every demo
//! [`RwSignal`], created once in [`Component::init`] and captured into the
//! stateless-exhibit builder closures — the catalog's `CatalogSignals` pattern,
//! moved one level down the tree (`docs/CODE_STANDARDS.md`'s State & Reactivity
//! Conventions carve-out for stateless-exhibit locals). The live design language
//! is read from `use_context::<Theme>()` (the settings toggle owns the override;
//! this screen only renders under whichever language is active), never
//! `set_app_theme`.

use forgekit::{
    AnyView, Button, Column, Component, CrossAxisAlignment, CupertinoButtonSize,
    CupertinoButtonStyle, DesignLanguage, FlexView, Get, ProgressValue, Row, RwSignal, Set,
    SizedBox, Stack, Switch, Theme, Update, any, assist_chip, button_group, circular_progress,
    component, cupertino_activity_indicator, cupertino_button, cupertino_switch, docked_toolbar,
    extended_fab, fab, fab_menu, fab_menu_item, filter_chip, floating_toolbar, linear_progress,
    loading_indicator, scroll_view, slider, split_button, text, use_context,
};

use crate::ShellState;

/// The Controls exhibit route entry point: a nested [`Component`] so the demo's
/// [`RwSignal`] state survives the router's per-rebuild page rebuild (see the
/// [module docs](self)).
pub fn controls_screen() -> AnyView<ShellState> {
    any(component(ControlsExhibit::default()))
}

/// The progress-indicator demo's determinate/indeterminate mode toggle.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum ProgressMode {
    #[default]
    Determinate,
    Indeterminate,
}

/// Every mutable exhibit signal the Controls builder reads. `Copy` (each field
/// is an [`RwSignal`]) so it captures cheaply into nested closures — the same
/// bundle shape `examples/catalog`'s `CatalogSignals` establishes.
#[derive(Clone, Copy)]
struct ControlsState {
    switch_on: RwSignal<bool>,
    filter_wifi: RwSignal<bool>,
    filter_bluetooth: RwSignal<bool>,
    assist_chip_taps: RwSignal<u32>,
    progress_mode: RwSignal<ProgressMode>,
    progress_value: RwSignal<f64>,
    slider_value: RwSignal<f64>,
    button_group_selected: RwSignal<usize>,
    split_button_open: RwSignal<bool>,
    split_button_presses: RwSignal<u32>,
    split_button_opens: RwSignal<u32>,
    fab_menu_open: RwSignal<bool>,
    fab_menu_last_selected: RwSignal<Option<String>>,
}

/// The Controls exhibit component: stateless configuration; all data lives in
/// its [`ControlsState`].
#[derive(Default)]
struct ControlsExhibit;

impl Component for ControlsExhibit {
    type State = ControlsState;

    fn init(&self) -> ControlsState {
        ControlsState {
            switch_on: RwSignal::new(false),
            filter_wifi: RwSignal::new(true),
            filter_bluetooth: RwSignal::new(false),
            assist_chip_taps: RwSignal::new(0),
            progress_mode: RwSignal::new(ProgressMode::default()),
            progress_value: RwSignal::new(0.35),
            slider_value: RwSignal::new(0.5),
            button_group_selected: RwSignal::new(0),
            split_button_open: RwSignal::new(false),
            split_button_presses: RwSignal::new(0),
            split_button_opens: RwSignal::new(0),
            fab_menu_open: RwSignal::new(false),
            fab_menu_last_selected: RwSignal::new(None),
        }
    }

    fn build(&self, state: &mut ControlsState) -> AnyView<ControlsState> {
        // Resolve the live design language from context (the settings toggle
        // owns the override; this screen only renders under it) — the same
        // read pattern `examples/catalog`'s `root_page` uses.
        let theme = use_context::<Theme>().unwrap_or_else(Theme::m3_baseline);
        let design = theme.design_language;
        let signals = *state;

        // The FAB menu (task 6f-08) fills its full box constraints, so it
        // overlays the scrolling exhibit rather than sitting inline in the
        // column — mirroring the catalog's `tab_content` Stack.
        any(Stack(vec![
            any(scroll_view(controls_column(design, signals))),
            fab_menu_overlay(signals),
        ]))
    }
}

/// The scrolling Controls exhibit column: every M3 + Cupertino control the
/// catalog's Controls tab shows.
fn controls_column(design: DesignLanguage, signals: ControlsState) -> FlexView<ControlsState> {
    let switch_on = signals.switch_on;
    let filter_wifi = signals.filter_wifi;
    let filter_bluetooth = signals.filter_bluetooth;
    let assist_chip_taps = signals.assist_chip_taps;
    let progress_mode = signals.progress_mode;
    let progress_value = signals.progress_value;

    let mut children: Vec<AnyView<ControlsState>> = Vec::new();
    children.push(any(text("Controls").size(24.0)));

    // -- Switch --------------------------------------------------------
    children.push(any(text(format!(
        "Switch: {}",
        if switch_on.get() { "on" } else { "off" }
    ))
    .size(14.0)));
    children.push(match design {
        DesignLanguage::Material3 => any(Switch(
            switch_on.get(),
            move |_s: &mut ControlsState, v: bool| switch_on.set(v),
        )),
        DesignLanguage::Cupertino => any(cupertino_switch(
            switch_on.get(),
            move |_s: &mut ControlsState, v: bool| switch_on.set(v),
        )),
    });

    // -- Chips (Material-only; no Cupertino equivalent, PLAN.md D5) -----
    children.push(any(text(
        "Chips (Material-only — no Cupertino equivalent, PLAN.md D5)",
    )
    .size(14.0)));
    children.push(any(Row(vec![
        any(assist_chip("Info", move |_s: &mut ControlsState| {
            assist_chip_taps.update(|n| *n += 1)
        })),
        any(SizedBox(Some(8.0), None)),
        any(filter_chip(
            "Wi-Fi",
            filter_wifi.get(),
            move |_s: &mut ControlsState, v: bool| filter_wifi.set(v),
        )),
        any(SizedBox(Some(8.0), None)),
        any(filter_chip(
            "Bluetooth",
            filter_bluetooth.get(),
            move |_s: &mut ControlsState, v: bool| filter_bluetooth.set(v),
        )),
    ])));
    children.push(any(text(format!(
        "Assist chip taps: {}",
        assist_chip_taps.get()
    ))
    .size(14.0)));

    // -- FAB (Material-only; no Cupertino equivalent, PLAN.md D5) -------
    children.push(any(text(
        "FAB (Material-only — no Cupertino equivalent, PLAN.md D5)",
    )
    .size(14.0)));
    children.push(any(Row(vec![
        any(fab(any(text("+").size(20.0)), |_s: &mut ControlsState| {}).label("Add")),
        any(SizedBox(Some(12.0), None)),
        any(extended_fab("Create", |_s: &mut ControlsState| {})),
    ])));

    // -- Progress --------------------------------------------------------
    children.push(any(text("Progress").size(18.0)));
    children.push(any(Row(vec![
        any(Button("Toggle mode", move |_s: &mut ControlsState| {
            progress_mode.update(|m| {
                *m = match *m {
                    ProgressMode::Determinate => ProgressMode::Indeterminate,
                    ProgressMode::Indeterminate => ProgressMode::Determinate,
                };
            });
        })),
        any(SizedBox(Some(8.0), None)),
        any(Button("Advance +10%", move |_s: &mut ControlsState| {
            progress_value.update(|v| {
                *v += 0.1;
                if *v > 1.0 {
                    *v -= 1.0;
                }
            });
        })),
    ])));
    let value = match progress_mode.get() {
        ProgressMode::Determinate => ProgressValue::Determinate(progress_value.get()),
        ProgressMode::Indeterminate => ProgressValue::Indeterminate,
    };
    children.push(any(linear_progress(value)));
    children.push(any(circular_progress(value)));
    if design == DesignLanguage::Cupertino {
        children.push(any(text("Cupertino activity indicator:").size(14.0)));
        children.push(any(cupertino_activity_indicator()));
    }

    // -- Wavy progress (M3 Expressive, task 6f-06) — same value/mode toggle
    // above, opted into `.wavy()` so the flat and wavy renders are driven
    // side by side by the same controls.
    children.push(any(
        text("Wavy progress (M3 Expressive, task 6f-06)").size(18.0)
    ));
    children.push(any(linear_progress(value).wavy()));
    children.push(any(circular_progress(value).wavy()));

    // -- Loading indicator (M3 Expressive, task 6f-05) — an ownerless,
    // continuously-morphing loop; nothing for the app to feed it.
    children.push(any(
        text("Loading indicator (M3 Expressive, task 6f-05)").size(18.0)
    ));
    children.push(any(loading_indicator()));

    // -- Button group (M3 Expressive, task 6f-07) -----------------------
    let button_group_selected = signals.button_group_selected;
    children.push(any(text("Button group (task 6f-07)").size(18.0)));
    children.push(any(button_group(
        ["Day", "Week", "Month"],
        button_group_selected.get(),
        move |_s: &mut ControlsState, idx: usize| button_group_selected.set(idx),
    )));
    children.push(any(text(format!(
        "Selected: {}",
        ["Day", "Week", "Month"][button_group_selected.get().min(2)]
    ))
    .size(14.0)));

    // -- Split button (M3 Expressive, task 6f-07) ------------------------
    let split_button_open = signals.split_button_open;
    let split_button_presses = signals.split_button_presses;
    let split_button_opens = signals.split_button_opens;
    children.push(any(text("Split button (task 6f-07)").size(18.0)));
    children.push(any(split_button(
        "Create",
        split_button_open.get(),
        move |_s: &mut ControlsState| split_button_presses.update(|n| *n += 1),
        move |_s: &mut ControlsState| {
            split_button_opens.update(|n| *n += 1);
            split_button_open.update(|o| *o = !*o);
        },
    )));
    children.push(any(text(format!(
        "Presses: {}  ·  Menu opens: {}",
        split_button_presses.get(),
        split_button_opens.get()
    ))
    .size(14.0)));

    // -- Toolbars (M3 Expressive, task 6f-09) ----------------------------
    children.push(any(text("Toolbars (task 6f-09)").size(18.0)));
    children.push(any(text("Floating toolbar:").size(14.0)));
    children.push(any(floating_toolbar::<ControlsState>()
        .leading(vec![any(Button("Search", |_s: &mut ControlsState| {}))])
        .trailing(vec![any(Button("Filter", |_s: &mut ControlsState| {}))])
        .fab(any(fab(
            any(text("+").size(18.0)),
            |_s: &mut ControlsState| {},
        )))));
    children.push(any(text("Docked toolbar:").size(14.0)));
    children.push(any(docked_toolbar::<ControlsState>()
        .leading(vec![any(Button("Undo", |_s: &mut ControlsState| {}))])
        .center(vec![any(text("Docked toolbar").size(14.0))])
        .trailing(vec![any(Button("Redo", |_s: &mut ControlsState| {}))])));

    // -- FAB menu (M3 Expressive, task 6f-08) — overlaid on this exhibit by
    // `build`'s Stack; this row just reports its last-selected item so the
    // exhibit reads without needing to trigger the overlay first.
    children.push(any(text(
        "FAB menu (task 6f-08) — bottom-right overlay on this screen",
    )
    .size(18.0)));
    children.push(any(text(format!(
        "Last selected: {}",
        signals
            .fab_menu_last_selected
            .get()
            .unwrap_or_else(|| "(none)".to_string())
    ))
    .size(14.0)));

    // -- Slider (Cupertino-reskinned per task 6f-13; the same widget
    // auto-branches its render on the live design language) -------------
    let slider_value = signals.slider_value;
    children.push(any(text("Slider").size(18.0)));
    children.push(any(slider(
        slider_value.get(),
        move |_s: &mut ControlsState, v: f64| slider_value.set(v),
    )));
    children.push(any(text(format!(
        "Slider value: {:.2}",
        slider_value.get()
    ))
    .size(14.0)));

    if design == DesignLanguage::Cupertino {
        // -- Glass navbar/tabbar (task 6f-11) ----------------------------
        children.push(any(text(
            "Glass navbar (top bar)/floating tabbar (bottom bar, task 6f-11) — \
             visible by default whenever Cupertino is active; see the shell's \
             persistent top/bottom bars.",
        )
        .size(14.0)));

        // -- Cupertino switch/slider re-skin metrics (task 6f-13) --------
        children.push(any(text(
            "Cupertino switch track: 64×28pt, kit-cited (task 6f-13) — glassy knob \
             highlight above.",
        )
        .size(12.0)));

        // -- Capsule buttons: size classes x Glass style (task 6f-12) ----
        children.push(any(text(
            "Capsule buttons: size classes + Glass style (task 6f-12)",
        )
        .size(18.0)));
        children.push(any(Row(vec![
            any(cupertino_button("Small", |_s: &mut ControlsState| {})
                .size(CupertinoButtonSize::Small)
                .style(CupertinoButtonStyle::Glass)),
            any(SizedBox(Some(8.0), None)),
            any(cupertino_button("Medium", |_s: &mut ControlsState| {})
                .size(CupertinoButtonSize::Medium)
                .style(CupertinoButtonStyle::Glass)),
            any(SizedBox(Some(8.0), None)),
            any(cupertino_button("Large", |_s: &mut ControlsState| {})
                .size(CupertinoButtonSize::Large)
                .style(CupertinoButtonStyle::Glass)),
        ])));
    }

    Column(children).cross_axis(CrossAxisAlignment::Stretch)
}

/// The Controls exhibit's [`fab_menu`] overlay (task 6f-08): a bottom-right
/// trigger revealing two demo items, laid over the scrolling content by
/// [`ControlsExhibit::build`]'s [`Stack`] — see [`fab_menu`]'s module docs for
/// why it needs a full-area host rather than sitting inline in a column.
fn fab_menu_overlay(signals: ControlsState) -> AnyView<ControlsState> {
    let open = signals.fab_menu_open;
    let last_selected = signals.fab_menu_last_selected;

    any(fab_menu(
        any(text("+").size(20.0)),
        open.get(),
        vec![
            fab_menu_item(
                any(text("A").size(16.0)),
                "Alpha action",
                move |_s: &mut ControlsState| last_selected.set(Some("Alpha action".to_string())),
            ),
            fab_menu_item(
                any(text("B").size(16.0)),
                "Beta action",
                move |_s: &mut ControlsState| last_selected.set(Some("Beta action".to_string())),
            ),
        ],
        move |_s: &mut ControlsState| open.update(|o| *o = !*o),
    )
    .label("Quick actions"))
}
