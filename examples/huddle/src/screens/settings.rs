//! Settings screen (wave-2 task 06) — the design-language + brightness
//! selectors that live-swap the whole app.
//!
//! A [`Component`] hosting a [`SettingsController`](crate::settings_domain)
//! (via `use_controller`, the same seam `TeamScreen` uses). The selectors are
//! built from the catalog's own widgets — a `button_group` (Material) or a row
//! of capsule `cupertino_button`s (Cupertino) for the language, and a `Switch`
//! / `cupertino_switch` for dark/light — so the settings UI is itself a
//! widget showcase, branched on the live [`DesignLanguage`] like the shell
//! chrome.
//!
//! Every selection change writes the controller's signal and drives
//! `controller.apply(current)` on the UI-thread local task queue
//! (`forgekit::spawn_local`, the `TeamScreen` pattern), which runs the
//! [`SetTheme`](crate::settings_domain::SetTheme) use case — the one place the
//! app calls `set_app_theme`/`clear_app_theme`. The swap reaches all four tabs
//! and survives navigation (the override is a process-global).
//!
//! # Transition-preset default
//!
//! The plan asks this screen to also own the router's page-transition default.
//! Task 02's `/settings` route builder is a bare `|_params| settings_screen()`
//! that is **not** handed the shared `ShellSignals` bundle (which carries
//! `transition`), and this task may not edit `routes.rs`. So the live
//! transition switcher stays in Showcase → Navigation playground (already
//! shipped in task 02, which does receive `signals`); this screen only points
//! at it. Wiring a settings-owned default would need the route seam to pass
//! `signals`, a `routes.rs` change out of this task's scope.

use std::sync::Arc;

use forgekit::{
    AnyView, Axis, Brightness, Button, Column, Component, CrossAxisAlignment, DesignLanguage,
    EdgeInsets, FlexView, Get, Padding, Row, Set, SizedBox, Switch, Theme, any, button_group,
    component, cupertino_button, cupertino_switch, inflexible, scroll_view, text, use_context,
};

use crate::ShellState;
use crate::failure::TeamFailure;
use crate::settings_domain::{BrightnessChoice, DesignChoice, SettingsController};
use clean_signals_forgekit::use_controller;

/// The settings screen's route entry point: a `Component` under any outer state
/// (here `ShellState`), so its `init` runs under a real reactive `Owner` that
/// disposes the hosted controller on teardown.
pub fn settings_screen() -> AnyView<ShellState> {
    any(component(SettingsScreen))
}

/// The settings screen `Component`. Stateless configuration; all state lives in
/// its [`SettingsState`].
struct SettingsScreen;

/// Retained state: just the hosted controller (its selection signals are the
/// screen's only mutable state — there are no text drafts here).
struct SettingsState {
    controller: Arc<SettingsController>,
}

impl Component for SettingsScreen {
    type State = SettingsState;

    fn init(&self) -> SettingsState {
        // Seed the selectors from the currently-active theme so a re-entered
        // screen reflects the live language/brightness (see the domain's
        // navigation-survival note — `System` is not re-derivable, so this
        // shows the concrete live values).
        let theme = use_context::<Theme>().unwrap_or_else(Theme::m3_baseline);
        let design0 = DesignChoice::from_design_language(theme.design_language);
        let brightness0 = BrightnessChoice::from_brightness(theme.brightness);

        let controller = use_controller::<SettingsController, TeamFailure>(move || {
            SettingsController::new(design0, brightness0)
        });

        SettingsState { controller }
    }

    fn build(&self, state: &mut SettingsState) -> AnyView<SettingsState> {
        // Tracked reads: a later `apply` writes these signals, waking the frame.
        let design_choice = state.controller.design.get();
        let brightness_choice = state.controller.brightness.get();

        // The ambient theme drives both the selector chrome (Material vs
        // Cupertino) and the `System`-axis resolution inside `apply`.
        let theme = use_context::<Theme>().unwrap_or_else(Theme::m3_baseline);
        let design_lang = theme.design_language;
        let effective_dark = theme.brightness == Brightness::Dark;

        let mut children: Vec<AnyView<SettingsState>> = Vec::new();
        children.push(any(text("Settings").size(32.0)));
        children.push(any(text(
            "Language + brightness swap the entire app and survive navigation.",
        )
        .size(13.0)));

        // -- Design-language selector ------------------------------------
        children.push(any(text(format!(
            "Design language: {}",
            design_choice.label()
        ))
        .size(16.0)));
        children.push(design_selector(design_lang, design_choice, theme.clone()));

        // -- Brightness selector -----------------------------------------
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
            move |st: &mut SettingsState| {
                st.controller.brightness.set(BrightnessChoice::System);
                spawn_apply(&st.controller, current.clone());
            }
        })));

        // -- Transition-preset pointer (see the module docs) --------------
        children.push(any(text(
            "Page-transition preset: adjust it in Showcase \u{2192} Navigation \
                 playground (this route is not handed the shared signal bundle).",
        )
        .size(12.0)));

        any(scroll_view(Padding(
            EdgeInsets::all(16.0),
            any(Column(children)),
        )))
    }
}

/// The language selector: a Material `button_group`, or a row of Cupertino
/// capsule buttons (there is no Cupertino connected-group widget). Selecting a
/// member records the choice and applies it.
fn design_selector(
    design_lang: DesignLanguage,
    selected: DesignChoice,
    current: Theme,
) -> AnyView<SettingsState> {
    match design_lang {
        DesignLanguage::Material3 => any(button_group::<SettingsState, _>(
            DesignChoice::ALL.into_iter().map(|c| c.label()),
            selected.index(),
            move |st: &mut SettingsState, idx: usize| {
                st.controller.design.set(DesignChoice::from_index(idx));
                spawn_apply(&st.controller, current.clone());
            },
        )),
        DesignLanguage::Cupertino => {
            let mut btns: Vec<AnyView<SettingsState>> = Vec::new();
            for (i, choice) in DesignChoice::ALL.into_iter().enumerate() {
                if i > 0 {
                    btns.push(any(SizedBox(Some(8.0), None)));
                }
                let current = current.clone();
                btns.push(any(cupertino_button(
                    choice.label(),
                    move |st: &mut SettingsState| {
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

/// The dark/light switch (`Switch`/`cupertino_switch`), reflecting the *live*
/// effective brightness rather than the choice, so it stays honest under a
/// `System` selection. Toggling forces an explicit Light/Dark choice.
fn brightness_switch(
    design_lang: DesignLanguage,
    effective_dark: bool,
    current: Theme,
) -> AnyView<SettingsState> {
    let on_toggle = move |st: &mut SettingsState, on: bool| {
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
/// `TeamScreen::init` `spawn_local` pattern. `apply` runs the `SetTheme` use
/// case, whose `set_app_theme`/`clear_app_theme` the shell picks up on its next
/// once-per-frame override poll.
fn spawn_apply(controller: &Arc<SettingsController>, current: Theme) {
    let handle = Arc::clone(controller);
    forgekit::spawn_local(async move {
        handle.apply(current).await;
    });
}
