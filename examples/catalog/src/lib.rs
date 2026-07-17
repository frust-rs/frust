//! Catalog — the Phase 6c widget-catalog exit-criterion demo (PLAN.md D5),
//! extended in Phase 6f (task 15) to exhibit every 6f design-modernization
//! addition (glass materials, the M3 Expressive additions, and the
//! Cupertino re-skins) alongside the original 6c catalog.
//!
//! A NavigationBar-scaffolded app exercising the whole Material/Cupertino
//! widget catalog (tasks 07-14 of Phase 6c, plus Phase 6f's tasks 05-14)
//! through the `forgekit` facade alone (no escape-hatch dependency —
//! `examples/navdemo` is the facade-only baseline this mirrors, per
//! `docs/CODE_STANDARDS.md`'s State & Reactivity Conventions).
//!
//! # Pages
//!
//! A bottom nav bar (Material [`NavigationBar`]/Cupertino [`CupertinoTabBar`],
//! branching on the live design language — see Theme toggle below) switches
//! between four tabs:
//!
//! * **Controls** — [`Switch`]/[`CupertinoSwitch`], assist/filter [`Chips`
//!   (`assist_chip`/`filter_chip`)](crate), a [`fab`]/[`extended_fab`],
//!   linear/circular [`ProgressValue`] indicators (determinate + indeterminate,
//!   flat and [`.wavy()`](crate::LinearProgressView::wavy) — task 6f-06), a
//!   [`loading_indicator`] (task 6f-05), a [`button_group`] (task 6f-07), a
//!   [`split_button`] (task 6f-07), a bottom-right [`fab_menu`] overlay
//!   (task 6f-08, see [`fab_menu_overlay`]), floating/docked
//!   [toolbars](crate::floating_toolbar) (task 6f-09), a [`Slider`]
//!   (Cupertino-reskinned per task 6f-13), and, in Cupertino mode, a glass
//!   navbar/tabbar note (task 6f-11) plus capsule [`cupertino_button`]s at
//!   every size class in [`CupertinoButtonStyle::Glass`] (task 6f-12).
//! * **Cards** — elevated/filled/outlined [`Card`](crate::card)s plus a
//!   1000-row [`ListView`] (`ListView::builder`), proving the windowed
//!   virtualization pattern (`docs/ARCHITECTURE.md`'s `ListView` precedent).
//! * **Modals** — four buttons driving [`show_dialog`]/[`show_bottom_sheet`]/
//!   [`show_cupertino_alert`]/[`show_action_sheet`] (the Cupertino pair now
//!   glass-styled, task 6f-14), each landing its [`PopResult`] in a "last
//!   result" banner.
//! * **AppBars** — an embedded [`app_bar`] and [`cupertino_nav_bar`] side by
//!   side (the Material title now the task 6f-10 emphasized `titleLarge`
//!   token), so both top-bar styles are visible regardless of the live
//!   design language.
//!
//! # Shape
//!
//! Mirroring `examples/navdemo`'s documented constraint: a [`navigator`]
//! page builder is a plain `Fn() -> AnyView<State>` captured once and
//! re-invoked on every subsequent rebuild with **no** `&State` argument (see
//! `forgekit_widgets::navigator`'s module docs). Since every tab's content
//! must react live to user interaction, the whole scaffold (top bar, tab
//! content, bottom bar) is built as the navigator's single page, reading
//! every piece of mutable UI state through an [`RwSignal`] (bundled as
//! [`CatalogSignals`], `Copy` so it captures cheaply into the page closure)
//! rather than a plain `AppState` field — the textbook case
//! `docs/CODE_STANDARDS.md`'s State & Reactivity Conventions carves out for
//! this. The navigator itself hosts the four `show_*` modal helpers, which
//! push a transparent page over this same scaffold.
//!
//! # Theme toggle
//!
//! The top bar's trailing button calls [`set_app_theme`] to force the other
//! design language's baseline (task 6c-04's override seam) — the *next*
//! rebuild reads the swapped theme back via `use_context::<Theme>()`
//! (`examples/gallery`'s established read pattern) and branches every
//! Material/Cupertino counterpart pair on `design_language`. Chips/FAB/Cards
//! have no Cupertino counterpart (documented, PLAN.md D5) and are shown
//! unconditionally on both design languages.
//!
//! The override is built as `Theme::cupertino_baseline().with_brightness(live)`
//! (and the M3 mirror), not the bare baseline constructor: both baselines
//! hardcode `Brightness::Light`, and the override-wins rule (spec) means a
//! bare `set_app_theme(Theme::m3_baseline())` would silently pin the app to
//! Light forever, ignoring further OS night-mode reports (6e Finding 6, bug
//! 2). [`top_bar`] reads the live brightness off the same [`Theme`]
//! `root_page` already pulled from `use_context` and threads it through
//! [`Theme::with_brightness`] so forcing a design language never discards it.

use forgekit::{
    AnyView, Axis, Brightness, Button, Column, Component, CrossAxisAlignment, CupertinoActionStyle,
    CupertinoButtonSize, CupertinoButtonStyle, DesignLanguage, FlexView, Get, NavigatorController,
    ONE_LINE_HEIGHT, PopResult, ProgressValue, Row, RwSignal, Set, SizedBox, Stack, Switch, Theme,
    Update, action, any, app_bar, assist_chip, bottom_sheet, button_group, circular_progress,
    cupertino_activity_indicator, cupertino_button, cupertino_nav_bar, cupertino_switch,
    cupertino_tab_bar, dialog, docked_toolbar, elevated_card, extended_fab, fab, fab_menu,
    fab_menu_item, filled_card, filter_chip, flexible, floating_toolbar, inflexible,
    linear_progress, list_item, list_view, loading_indicator, nav_item, navigation_bar, navigator,
    outlined_card, scroll_view, set_app_theme, show_action_sheet, show_bottom_sheet,
    show_cupertino_alert, show_dialog, slider, split_button, tab_item, text, use_context,
};

/// Which top-level page the bottom nav bar currently selects.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Tab {
    #[default]
    Controls,
    Cards,
    Modals,
    AppBars,
}

impl Tab {
    /// Every tab, in display/nav-bar order.
    const ALL: [Tab; 4] = [Tab::Controls, Tab::Cards, Tab::Modals, Tab::AppBars];

    fn label(self) -> &'static str {
        match self {
            Tab::Controls => "Controls",
            Tab::Cards => "Cards",
            Tab::Modals => "Modals",
            Tab::AppBars => "AppBars",
        }
    }

    fn index(self) -> usize {
        Tab::ALL
            .iter()
            .position(|t| *t == self)
            .expect("self is always a member of ALL")
    }

    fn from_index(idx: usize) -> Self {
        Tab::ALL.get(idx).copied().unwrap_or_default()
    }
}

/// The Controls tab's progress-indicator demo mode.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum ProgressMode {
    #[default]
    Determinate,
    Indeterminate,
}

/// Every piece of mutable UI state the navigator's page-builder closure
/// reads (see the [module docs](self)'s Shape section for why this can't
/// just be `AppState` fields read by reference). `Copy` (every field is an
/// `RwSignal`) so it captures cheaply into nested closures.
#[derive(Clone, Copy)]
struct CatalogSignals {
    tab: RwSignal<Tab>,
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
    selected_row: RwSignal<Option<usize>>,
    last_modal_result: RwSignal<Option<String>>,
}

/// Catalog's retained state (spec §5's `app_logic` model, phase 5.5's
/// `Component::State`): the navigator handle plus the [`CatalogSignals`]
/// bundle every tab reads/writes.
pub struct AppState {
    controller: NavigatorController<AppState>,
    signals: CatalogSignals,
}

/// The generated app's root [`Component`] (spec §5.5): stateless config
/// wiring [`AppState`]'s navigator + tab scaffold together.
#[derive(Default)]
pub struct CatalogApp;

impl Component for CatalogApp {
    type State = AppState;

    fn init(&self) -> AppState {
        AppState {
            controller: NavigatorController::new(),
            signals: CatalogSignals {
                tab: RwSignal::new(Tab::default()),
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
                selected_row: RwSignal::new(None),
                last_modal_result: RwSignal::new(None),
            },
        }
    }

    fn build(&self, state: &mut AppState) -> AnyView<AppState> {
        let controller = state.controller.clone();
        let initial_controller = controller.clone();
        let signals = state.signals;

        any(navigator(&controller, move || {
            root_page(initial_controller.clone(), signals)
        }))
    }
}

/// Build the whole app scaffold — top bar, the selected tab's content
/// (flexibly sized to fill the space between the bars), and the bottom nav
/// bar — as the navigator's single page. See the [module docs](self)'s Shape
/// section for why this reads every mutable field through `signals` rather
/// than a borrowed `&AppState`.
fn root_page(
    controller: NavigatorController<AppState>,
    signals: CatalogSignals,
) -> AnyView<AppState> {
    // `use_context` resolves here because the navigator's page-builder
    // closure runs inside `root.rebuild(..)`, itself run under the shell's
    // `runtime.with_owner(|| scope.track(..))` (see
    // `docs/ARCHITECTURE.md`'s Frame pipeline) — the same read pattern
    // `examples/gallery`'s `GalleryApp::build` uses.
    let theme = use_context::<Theme>().unwrap_or_else(Theme::m3_baseline);
    let design = theme.design_language;
    let brightness = theme.brightness;

    let top = top_bar(design, brightness);
    let bottom = bottom_bar(design, signals.tab);
    let content = tab_content(design, signals.tab.get(), controller, signals);

    any(FlexView::new(
        Axis::Vertical,
        vec![inflexible(top), flexible(1, content), inflexible(bottom)],
    )
    .cross_axis(CrossAxisAlignment::Stretch))
}

/// The persistent top bar: [`app_bar`] (Material) or [`cupertino_nav_bar`]
/// (Cupertino), each with a trailing button that forces the *other* design
/// language's baseline via [`set_app_theme`] (task 6c-04's override seam).
///
/// Takes the live `brightness` alongside `design` and threads it through
/// [`Theme::with_brightness`] when building the override (6e Finding 6, bug
/// 2 fix): `Theme::cupertino_baseline()`/`Theme::m3_baseline()` alone
/// hardcode `Brightness::Light`, which would silently pin the app to Light
/// under the override-wins rule regardless of the OS's live appearance —
/// `with_brightness` forces the design language without discarding it.
fn top_bar(design: DesignLanguage, brightness: Brightness) -> AnyView<AppState> {
    match design {
        DesignLanguage::Material3 => {
            any(
                app_bar::<AppState>("ForgeKit Catalog").actions(vec![any(Button(
                    "Cupertino",
                    move |_s: &mut AppState| {
                        set_app_theme(Theme::cupertino_baseline().with_brightness(brightness))
                    },
                ))]),
            )
        }
        DesignLanguage::Cupertino => any(cupertino_nav_bar::<AppState>("ForgeKit Catalog")
            .trailing(any(Button("Material", move |_s: &mut AppState| {
                set_app_theme(Theme::m3_baseline().with_brightness(brightness))
            })))),
    }
}

/// The persistent bottom bar: [`navigation_bar`] (Material) or
/// [`cupertino_tab_bar`] (Cupertino), switching `tab` on selection.
fn bottom_bar(design: DesignLanguage, tab: RwSignal<Tab>) -> AnyView<AppState> {
    let selected = tab.get().index();
    match design {
        DesignLanguage::Material3 => any(navigation_bar::<AppState, _>(
            Tab::ALL
                .iter()
                .map(|t| nav_item::<AppState>(t.label()))
                .collect(),
            selected,
            move |_s: &mut AppState, idx: usize| tab.set(Tab::from_index(idx)),
        )),
        DesignLanguage::Cupertino => any(cupertino_tab_bar::<AppState, _>(
            Tab::ALL
                .iter()
                .map(|t| tab_item::<AppState>(t.label()))
                .collect(),
            selected,
            move |_s: &mut AppState, idx: usize| tab.set(Tab::from_index(idx)),
        )),
    }
}

/// Dispatch to the selected tab's content, sized to fill the space between
/// the top/bottom bars (see [`root_page`]'s flexible middle slot).
fn tab_content(
    design: DesignLanguage,
    tab: Tab,
    controller: NavigatorController<AppState>,
    signals: CatalogSignals,
) -> AnyView<AppState> {
    match tab {
        // The FAB menu (task 6f-08) fills its full box constraints (see its
        // module docs — it's "meant to be the top layer of a full-area
        // `Stack`"), so it overlays the scrolling content rather than sitting
        // inline in the column.
        Tab::Controls => any(Stack(vec![
            any(scroll_view(controls_tab(design, signals))),
            fab_menu_overlay(signals),
        ])),
        Tab::Cards => cards_tab(signals),
        Tab::Modals => any(scroll_view(modals_tab(
            controller,
            signals.last_modal_result,
        ))),
        Tab::AppBars => any(scroll_view(appbars_tab())),
    }
}

/// The Controls tab: a [`Switch`]/[`CupertinoSwitch`], assist/filter chips,
/// a [`fab`]/[`extended_fab`], and linear/circular progress indicators.
fn controls_tab(design: DesignLanguage, signals: CatalogSignals) -> FlexView<AppState> {
    let switch_on = signals.switch_on;
    let filter_wifi = signals.filter_wifi;
    let filter_bluetooth = signals.filter_bluetooth;
    let assist_chip_taps = signals.assist_chip_taps;
    let progress_mode = signals.progress_mode;
    let progress_value = signals.progress_value;

    let mut children: Vec<AnyView<AppState>> = Vec::new();
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
            move |_s: &mut AppState, v: bool| switch_on.set(v),
        )),
        DesignLanguage::Cupertino => any(cupertino_switch(
            switch_on.get(),
            move |_s: &mut AppState, v: bool| switch_on.set(v),
        )),
    });

    // -- Chips (Material-only; no Cupertino equivalent, PLAN.md D5) -----
    children.push(any(text(
        "Chips (Material-only — no Cupertino equivalent, PLAN.md D5)",
    )
    .size(14.0)));
    children.push(any(Row(vec![
        any(assist_chip("Info", move |_s: &mut AppState| {
            assist_chip_taps.update(|n| *n += 1)
        })),
        any(SizedBox(Some(8.0), None)),
        any(filter_chip(
            "Wi-Fi",
            filter_wifi.get(),
            move |_s: &mut AppState, v: bool| filter_wifi.set(v),
        )),
        any(SizedBox(Some(8.0), None)),
        any(filter_chip(
            "Bluetooth",
            filter_bluetooth.get(),
            move |_s: &mut AppState, v: bool| filter_bluetooth.set(v),
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
        any(fab(any(text("+").size(20.0)), |_s: &mut AppState| {}).label("Add")),
        any(SizedBox(Some(12.0), None)),
        any(extended_fab("Create", |_s: &mut AppState| {})),
    ])));

    // -- Progress --------------------------------------------------------
    children.push(any(text("Progress").size(18.0)));
    children.push(any(Row(vec![
        any(Button("Toggle mode", move |_s: &mut AppState| {
            progress_mode.update(|m| {
                *m = match *m {
                    ProgressMode::Determinate => ProgressMode::Indeterminate,
                    ProgressMode::Indeterminate => ProgressMode::Determinate,
                };
            });
        })),
        any(SizedBox(Some(8.0), None)),
        any(Button("Advance +10%", move |_s: &mut AppState| {
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
    // above, opted into `.wavy()` so both the flat and wavy renders are
    // driven side by side by the same controls.
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
        move |_s: &mut AppState, idx: usize| button_group_selected.set(idx),
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
        move |_s: &mut AppState| split_button_presses.update(|n| *n += 1),
        move |_s: &mut AppState| {
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
    children.push(any(floating_toolbar::<AppState>()
        .leading(vec![any(Button("Search", |_s: &mut AppState| {}))])
        .trailing(vec![any(Button("Filter", |_s: &mut AppState| {}))])
        .fab(any(fab(any(text("+").size(18.0)), |_s: &mut AppState| {})))));
    children.push(any(text("Docked toolbar:").size(14.0)));
    children.push(any(docked_toolbar::<AppState>()
        .leading(vec![any(Button("Undo", |_s: &mut AppState| {}))])
        .center(vec![any(text("Docked toolbar").size(14.0))])
        .trailing(vec![any(Button("Redo", |_s: &mut AppState| {}))])));

    // -- FAB menu (M3 Expressive, task 6f-08) — overlaid on this tab's
    // content by `tab_content` (see its Stack wrapping); this row just
    // reports its last-selected item so the exhibit reads without needing to
    // trigger the overlay first.
    children.push(any(text(
        "FAB menu (task 6f-08) — bottom-right overlay on this tab",
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
        move |_s: &mut AppState, v: f64| slider_value.set(v),
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
             visible by default whenever Cupertino is active; see the scaffold's \
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
            any(cupertino_button("Small", |_s: &mut AppState| {})
                .size(CupertinoButtonSize::Small)
                .style(CupertinoButtonStyle::Glass)),
            any(SizedBox(Some(8.0), None)),
            any(cupertino_button("Medium", |_s: &mut AppState| {})
                .size(CupertinoButtonSize::Medium)
                .style(CupertinoButtonStyle::Glass)),
            any(SizedBox(Some(8.0), None)),
            any(cupertino_button("Large", |_s: &mut AppState| {})
                .size(CupertinoButtonSize::Large)
                .style(CupertinoButtonStyle::Glass)),
        ])));
    }

    Column(children).cross_axis(CrossAxisAlignment::Stretch)
}

/// The Controls tab's [`fab_menu`] overlay (task 6f-08): a bottom-right
/// trigger revealing two demo items, laid over [`controls_tab`]'s scrolling
/// content by [`tab_content`]'s [`Stack`] — see [`fab_menu`]'s module docs for
/// why it needs a full-area host rather than sitting inline in a column.
fn fab_menu_overlay(signals: CatalogSignals) -> AnyView<AppState> {
    let open = signals.fab_menu_open;
    let last_selected = signals.fab_menu_last_selected;

    any(fab_menu(
        any(text("+").size(20.0)),
        open.get(),
        vec![
            fab_menu_item(
                any(text("A").size(16.0)),
                "Alpha action",
                move |_s: &mut AppState| last_selected.set(Some("Alpha action".to_string())),
            ),
            fab_menu_item(
                any(text("B").size(16.0)),
                "Beta action",
                move |_s: &mut AppState| last_selected.set(Some("Beta action".to_string())),
            ),
        ],
        move |_s: &mut AppState| open.update(|o| *o = !*o),
    )
    .label("Quick actions"))
}

/// The Cards tab: elevated/filled/outlined [`card`] previews plus a
/// 1000-row virtualized [`list_view`] proving the windowed materialization
/// pattern (`docs/ARCHITECTURE.md`'s `ListView` precedent) — a row press
/// updates the "Selected row" banner above it.
fn cards_tab(signals: CatalogSignals) -> AnyView<AppState> {
    let selected_row = signals.selected_row;

    let header = Column(vec![
        any(text("Cards + ListView").size(24.0)),
        any(Row(vec![
            any(elevated_card(text("Elevated").size(16.0))),
            any(SizedBox(Some(8.0), None)),
            any(filled_card(text("Filled").size(16.0))),
            any(SizedBox(Some(8.0), None)),
            any(outlined_card(text("Outlined").size(16.0))),
        ])),
        any(text(format!(
            "Selected row: {}",
            selected_row
                .get()
                .map(|i| i.to_string())
                .unwrap_or_else(|| "(none)".to_string())
        ))
        .size(14.0)),
        any(text("1000-row virtualized list:").size(14.0)),
    ]);

    let list = list_view::<AppState>(1000, ONE_LINE_HEIGHT, move |i| {
        any(list_item::<AppState>(format!("Row {i}"))
            .supporting(format!("Item index {i}"))
            .on_press(move |_s: &mut AppState| selected_row.set(Some(i))))
    });

    any(
        FlexView::new(Axis::Vertical, vec![inflexible(header), flexible(1, list)])
            .cross_axis(CrossAxisAlignment::Stretch),
    )
}

/// The Modals tab: buttons driving each of the four modal helpers, with a
/// "Last result" banner reflecting the most recent [`PopResult`].
fn modals_tab(
    controller: NavigatorController<AppState>,
    last_modal_result: RwSignal<Option<String>>,
) -> FlexView<AppState> {
    let dialog_controller = controller.clone();
    let sheet_controller = controller.clone();
    let alert_controller = controller.clone();
    let action_sheet_controller = controller;

    Column(vec![
        any(text("Modals").size(24.0)),
        any(text(format!(
            "Last result: {}",
            last_modal_result
                .get()
                .unwrap_or_else(|| "(none)".to_string())
        ))
        .size(16.0)),
        any(text(
            "Cupertino Alert/Action Sheet now render on the iOS-27 glass \
             material (task 6f-14) — trigger them below to see it.",
        )
        .size(12.0)),
        any(Button("Show Dialog", move |_s: &mut AppState| {
            let confirm_ctrl = dialog_controller.clone();
            let cancel_ctrl = dialog_controller.clone();
            show_dialog(
                &dialog_controller,
                move || {
                    let confirm_ctrl = confirm_ctrl.clone();
                    let cancel_ctrl = cancel_ctrl.clone();
                    dialog::<AppState>()
                        .title("Delete item?")
                        .body("This action can't be undone.")
                        .action(any(Button("Cancel", move |_s: &mut AppState| {
                            cancel_ctrl.pop();
                        })))
                        .action(any(Button("Confirm", move |_s: &mut AppState| {
                            confirm_ctrl.pop_with_result(PopResult::of("confirmed".to_string()));
                        })))
                },
                move |_s: &mut AppState, result: PopResult| {
                    let msg = result
                        .take::<String>()
                        .unwrap_or_else(|| "dismissed".to_string());
                    last_modal_result.set(Some(format!("Dialog: {msg}")));
                },
            );
        })),
        any(SizedBox(None, Some(8.0))),
        any(Button("Show Bottom Sheet", move |_s: &mut AppState| {
            let close_ctrl = sheet_controller.clone();
            show_bottom_sheet(
                &sheet_controller,
                move || {
                    let close_ctrl = close_ctrl.clone();
                    bottom_sheet(Column(vec![
                        any(text("Bottom sheet content").size(18.0)),
                        any(Button("Close", move |_s: &mut AppState| close_ctrl.pop())),
                    ]))
                },
                move |_s: &mut AppState, _result: PopResult| {
                    last_modal_result.set(Some("Bottom sheet dismissed".to_string()));
                },
            );
        })),
        any(SizedBox(None, Some(8.0))),
        any(Button("Show Cupertino Alert", move |_s: &mut AppState| {
            show_cupertino_alert(
                &alert_controller,
                "iOS Alert",
                Some("This is a Cupertino-style alert.".to_string()),
                vec![
                    action("Cancel").style(CupertinoActionStyle::Cancel),
                    action("Delete").style(CupertinoActionStyle::Destructive),
                ],
                move |_s: &mut AppState, result: PopResult| {
                    let idx = result.take::<usize>();
                    last_modal_result.set(Some(format!("Cupertino alert action: {idx:?}")));
                },
            );
        })),
        any(SizedBox(None, Some(8.0))),
        any(Button("Show Action Sheet", move |_s: &mut AppState| {
            show_action_sheet(
                &action_sheet_controller,
                vec![
                    action("Share"),
                    action("Delete").style(CupertinoActionStyle::Destructive),
                ],
                Some("Cancel".to_string()),
                move |_s: &mut AppState, result: PopResult| {
                    let idx = result.take::<usize>();
                    last_modal_result.set(Some(format!("Action sheet action: {idx:?}")));
                },
            );
        })),
    ])
}

/// The AppBars tab: a Material [`app_bar`] and a Cupertino
/// [`cupertino_nav_bar`] side by side, so both are visible regardless of the
/// live design language.
fn appbars_tab() -> FlexView<AppState> {
    Column(vec![
        any(text("AppBars").size(24.0)),
        any(text(
            "Emphasized type specimen (task 6f-10): the title below uses M3 \
             Expressive's titleLarge-emphasized token (Medium weight) instead \
             of the plain baseline titleLarge.",
        )
        .size(12.0)),
        any(text("Material top AppBar (small, center-aligned):").size(14.0)),
        any(app_bar::<AppState>("Section header")),
        any(SizedBox(None, Some(16.0))),
        any(text("Cupertino top NavBar:").size(14.0)),
        any(cupertino_nav_bar::<AppState>("Section header")),
    ])
}

// The demo's sole entry point (spec §5.5/§10): one line binds `CatalogApp` to
// all three platforms — the desktop preview loop (via the hidden
// `__forgekit_main` `main.rs` calls), the Android JNI exports, and the iOS
// C-ABI exports.
forgekit::app!(CatalogApp);

#[cfg(test)]
mod tests {
    use super::*;
    use forgekit::TransitionSpec;
    use forgekit_core::{InputEvent, PointerButton, PointerEvent, PointerPhase, RenderRoot};
    use forgekit_reactive::ReactiveRuntime;
    use kurbo::Point;
    use std::sync::Arc;

    // A single test function, deliberately: `ReactiveRuntime::init` swaps a
    // process-wide waker (see `examples/navdemo`'s identical single-test-fn
    // rationale) — two `#[test]` fns on the parallel test runner's separate
    // threads would race each other on that shared state.
    #[test]
    fn catalog_builds_switches_tabs_and_completes_a_modal_round_trip() {
        let _rt = ReactiveRuntime::init(Arc::new(|| {}));

        let mut root: RenderRoot<AppState, AnyView<AppState>> = RenderRoot::new();
        let mut state = CatalogApp.init();
        let mut logic = |s: &mut AppState| CatalogApp.build(s);

        // Criterion: the initial scaffold (Controls tab, Material design)
        // builds with no panic.
        root.rebuild(&mut logic, &mut state);
        assert!(root.root_id().is_some(), "the root scaffold must build");

        // Criterion: switching every tab rebuilds without panicking — each
        // arm of `tab_content`'s match, including the 1000-row `ListView`,
        // actually runs.
        for tab in Tab::ALL {
            state.signals.tab.set(tab);
            root.rebuild(&mut logic, &mut state);
            assert!(
                root.root_id().is_some(),
                "the scaffold must rebuild on tab {tab:?}"
            );
        }

        // Criterion: a live Material -> Cupertino theme swap (the toolbar
        // toggle's effect) rebuilds without panicking, exercising every
        // Material/Cupertino branch in the scaffold.
        forgekit::provide_context(Theme::cupertino_baseline());
        root.rebuild(&mut logic, &mut state);
        assert!(
            root.root_id().is_some(),
            "the scaffold must rebuild under the Cupertino baseline"
        );

        // Criterion (the state-dependent assertion, not a pure helper): a
        // modal pushed transparently through the *real*, mounted
        // `NavigatorController` and popped with a result must deliver that
        // `PopResult` back into app state via `on_result` — this only holds
        // if the navigator's op-queue-drain/rebuild pipeline actually ran,
        // not merely that a signal was set and read back.
        let last_modal_result = state.signals.last_modal_result;
        state.controller.push_transparent_for_result(
            || any(text("probe dialog")),
            TransitionSpec::NONE,
            move |_s: &mut AppState, result: PopResult| {
                last_modal_result.set(result.take::<String>());
            },
        );
        root.rebuild(&mut logic, &mut state); // the pushed page's first build
        state
            .controller
            .pop_with_result(PopResult::of("confirmed".to_string()));
        root.rebuild(&mut logic, &mut state); // the structural pop applies here
        assert_eq!(
            last_modal_result.get(),
            None,
            "on_result is queued, not yet flushed (no event pass)"
        );
        // The next event pass flushes the queued on_result callback with
        // `&mut State` (mirrors `nav::navigator`'s own
        // `pop_result_reaches_callback_with_state` test).
        root.event(
            &mut state,
            &InputEvent::Pointer(PointerEvent {
                phase: PointerPhase::Move,
                position: Point::new(5.0, 5.0),
                button: PointerButton::Primary,
            }),
        );
        assert_eq!(
            last_modal_result.get(),
            Some("confirmed".to_string()),
            "a modal push/pop round trip through the real NavigatorController \
             must deliver its PopResult back into app state"
        );
    }
}
