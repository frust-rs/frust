//! Appearance settings (`/you/settings/appearance`) — the design-language +
//! brightness selectors that live-swap the whole app.
//!
//! This is the **one non-placeholder screen in the skeleton**: it hosts a
//! [`SettingsController`](crate::features::settings) (via `use_controller`, the
//! same seam the old team roster used) and is the single place the app calls
//! `set_app_theme`/`clear_app_theme`. The swap reaches all four tabs and
//! survives navigation (the override is a process-global — see
//! `docs/ARCHITECTURE.md`'s Theme delivery). Phase C's appearance task extends
//! this with accent themes + a dynamic-type slider; the `SettingsController` →
//! `SetTheme` mechanism and its tests stay put.

use std::sync::Arc;

use forgekit::{
    AnyView, Axis, Brightness, Button, Column, CrossAxisAlignment, DesignLanguage, EdgeInsets,
    FlexView, Get, Padding, Row, Set, SizedBox, Switch, Theme, any, app_bar, button_group,
    component, cupertino_button, cupertino_switch, flexible, inflexible, scroll_view, text,
    use_context,
};

use crate::HuddleState;
use crate::failure::HuddleFailure;
use crate::features::settings::{BrightnessChoice, DesignChoice, SettingsController};
use clean_signals_forgekit::use_controller;

/// The appearance screen's route entry point: a `Component` under the outer
/// [`HuddleState`], so its `init` runs under a real reactive `Owner` that
/// disposes the hosted controller on teardown.
pub fn appearance_screen() -> AnyView<HuddleState> {
    any(component(AppearanceScreen))
}

/// The appearance screen `Component`. Stateless configuration; all state lives
/// in its [`AppearanceState`].
struct AppearanceScreen;

/// Retained state: just the hosted controller (its selection signals are the
/// screen's only mutable state).
struct AppearanceState {
    controller: Arc<SettingsController>,
}

impl forgekit::Component for AppearanceScreen {
    type State = AppearanceState;

    fn init(&self) -> AppearanceState {
        // Seed the selectors from the currently-active theme so a re-entered
        // screen reflects the live language/brightness.
        let theme = use_context::<Theme>().unwrap_or_else(Theme::m3_baseline);
        let design0 = DesignChoice::from_design_language(theme.design_language);
        let brightness0 = BrightnessChoice::from_brightness(theme.brightness);

        let controller = use_controller::<SettingsController, HuddleFailure>(move || {
            SettingsController::new(design0, brightness0)
        });

        AppearanceState { controller }
    }

    fn build(&self, state: &mut AppearanceState) -> AnyView<AppearanceState> {
        // Tracked reads: a later `apply` writes these signals, waking the frame.
        let design_choice = state.controller.design.get();
        let brightness_choice = state.controller.brightness.get();

        let theme = use_context::<Theme>().unwrap_or_else(Theme::m3_baseline);
        let design_lang = theme.design_language;
        let effective_dark = theme.brightness == Brightness::Dark;

        let mut children: Vec<AnyView<AppearanceState>> = Vec::new();
        children.push(any(text("Appearance").size(28.0)));
        children.push(any(text(
            "Language + brightness swap the entire app and survive navigation.",
        )
        .size(13.0)));

        children.push(any(text(format!(
            "Design language: {}",
            design_choice.label()
        ))
        .size(16.0)));
        children.push(design_selector(design_lang, design_choice, theme.clone()));

        children.push(any(text(format!(
            "Brightness: {}",
            brightness_choice.label()
        ))
        .size(16.0)));
        children.push(brightness_switch(
            design_lang,
            effective_dark,
            theme.clone(),
        ));
        children.push(any(Button("Follow system brightness", {
            let current = theme.clone();
            move |st: &mut AppearanceState| {
                st.controller.brightness.set(BrightnessChoice::System);
                spawn_apply(&st.controller, current.clone());
            }
        })));

        any(FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(any(app_bar::<AppearanceState>("Appearance"))),
                flexible(
                    1,
                    any(scroll_view(Padding(
                        EdgeInsets::all(16.0),
                        Column(children),
                    ))),
                ),
            ],
        )
        .cross_axis(CrossAxisAlignment::Stretch))
    }
}

/// The language selector: a Material `button_group`, or a row of Cupertino
/// capsule buttons (there is no Cupertino connected-group widget).
fn design_selector(
    design_lang: DesignLanguage,
    selected: DesignChoice,
    current: Theme,
) -> AnyView<AppearanceState> {
    match design_lang {
        DesignLanguage::Material3 => any(button_group::<AppearanceState, _>(
            DesignChoice::ALL.into_iter().map(|c| c.label()),
            selected.index(),
            move |st: &mut AppearanceState, idx: usize| {
                st.controller.design.set(DesignChoice::from_index(idx));
                spawn_apply(&st.controller, current.clone());
            },
        )),
        DesignLanguage::Cupertino => {
            let mut btns: Vec<AnyView<AppearanceState>> = Vec::new();
            for (i, choice) in DesignChoice::ALL.into_iter().enumerate() {
                if i > 0 {
                    btns.push(any(SizedBox(Some(8.0), None)));
                }
                let current = current.clone();
                btns.push(any(cupertino_button(
                    choice.label(),
                    move |st: &mut AppearanceState| {
                        st.controller.design.set(choice);
                        spawn_apply(&st.controller, current.clone());
                    },
                )));
            }
            any(FlexView::new(
                Axis::Horizontal,
                btns.into_iter().map(inflexible).collect::<Vec<_>>(),
            )
            .cross_axis(CrossAxisAlignment::Center))
        }
    }
}

/// The dark/light switch, reflecting the *live* effective brightness rather than
/// the choice, so it stays honest under a `System` selection.
fn brightness_switch(
    design_lang: DesignLanguage,
    effective_dark: bool,
    current: Theme,
) -> AnyView<AppearanceState> {
    let on_toggle = move |st: &mut AppearanceState, on: bool| {
        st.controller.brightness.set(if on {
            BrightnessChoice::Dark
        } else {
            BrightnessChoice::Light
        });
        spawn_apply(&st.controller, current.clone());
    };
    match design_lang {
        DesignLanguage::Material3 => any(Row(vec![
            any(text("Dark").size(14.0)),
            any(SizedBox(Some(8.0), None)),
            any(Switch(effective_dark, on_toggle)),
        ])),
        DesignLanguage::Cupertino => any(Row(vec![
            any(text("Dark").size(14.0)),
            any(SizedBox(Some(8.0), None)),
            any(cupertino_switch(effective_dark, on_toggle)),
        ])),
    }
}

/// Drive `controller.apply(current)` on the UI-thread local task queue — the
/// `spawn_local` pattern. `apply` runs the `SetTheme` use case, whose
/// `set_app_theme`/`clear_app_theme` the shell picks up on its next
/// once-per-frame override poll.
fn spawn_apply(controller: &Arc<SettingsController>, current: Theme) {
    let handle = Arc::clone(controller);
    forgekit::spawn_local(async move {
        handle.apply(current).await;
    });
}
