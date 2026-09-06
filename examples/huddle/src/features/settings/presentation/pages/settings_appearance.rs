//! Appearance settings (`/you/settings/appearance`) — the full theming engine:
//! brightness + design language, **3 accent themes**, **dynamic type scaling**,
//! and the ~200ms fade veil around every swap.
//!
//! This is the single place the app calls `set_app_theme`/`clear_app_theme` (via
//! the hosted [`SettingsController`] → `SetTheme` use case). Every selector on
//! this screen writes a controller signal and then re-runs `apply`, so the swap
//! reaches all four tabs at once and survives navigation (the override is a
//! process-global — see `docs/ARCHITECTURE.md`'s Theme delivery).
//!
//! # Fade veil scope (honest note)
//!
//! The veil is an **app-level overlay mounted inside this screen's own subtree**
//! (a `Stack` top layer), not the shell's frozen overlay slot. Every theme swap
//! originates here, so covering this screen's swap frame is where the fade reads.
//! A veil painted by a widget advancing an `AnimationController` off
//! `PaintCtx::frame_time` would be the framework-native path, but that is
//! below-facade and unavailable to app code; the veil is instead driven from the
//! controller off a synthetic clock (see [`SettingsController::trigger_veil`])
//! into a signal this screen paints. Other screens are not covered by the veil —
//! by design, since swaps don't originate there.

use std::sync::Arc;

use frust::{
    Align, Alignment, AnyView, Axis, Brightness, Button, Color, Column, CrossAxisAlignment,
    DesignLanguage, EdgeInsets, FlexView, Get, Image, ImageFit, Padding, Row, Set, SizedBox, Stack,
    Theme, any, component, flexible, inflexible, scroll_view, slider, text, use_context,
};
use frust_cupertino::{cupertino_button, cupertino_switch};
use frust_material::{CardVariant, Switch, app_bar, button_group, card};

use crate::HuddleState;
use crate::failure::HuddleFailure;
use crate::features::settings::{
    AccentChoice, BrightnessChoice, DesignChoice, SettingsController, slider_to_type_factor,
    type_factor_to_slider,
};
use crate::ui::solid_source::{solid_source, solid_source_alpha};
use clean_signals_frust::use_controller;

/// The appearance screen's route entry point: a `Component` under the outer
/// [`HuddleState`], so its `init` runs under a real reactive `Owner` that
/// disposes the hosted controller on teardown.
pub fn appearance_screen() -> AnyView<HuddleState> {
    any(component(AppearanceScreen))
}

/// The appearance screen `Component`. Stateless configuration; all state lives
/// in its [`AppearanceState`].
struct AppearanceScreen;

/// Retained state: the hosted controller (its selection signals + veil signal are
/// the screen's only mutable state).
struct AppearanceState {
    controller: Arc<SettingsController>,
}

impl frust::Component for AppearanceScreen {
    type State = AppearanceState;

    fn init(&self) -> AppearanceState {
        // Seed the brightness/design selectors from the currently-active theme so
        // a re-entered screen reflects the live language/brightness (accent + type
        // scale seed from their defaults — see `SettingsController::new`).
        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let design0 = DesignChoice::from_design_language(theme.design_language);
        let brightness0 = BrightnessChoice::from_brightness(theme.brightness);

        let controller = use_controller::<SettingsController, HuddleFailure>(move || {
            SettingsController::new(design0, brightness0)
        });

        AppearanceState { controller }
    }

    fn build(&self, state: &mut AppearanceState) -> AnyView<AppearanceState> {
        // Tracked reads: a later `apply`/veil write wakes the frame.
        let design_choice = state.controller.design.get();
        let brightness_choice = state.controller.brightness.get();
        let accent_choice = state.controller.accent.get();
        let type_factor = state.controller.type_factor.get();
        let veil_opacity = state.controller.veil.get();

        let theme = use_context::<Theme>().unwrap_or_else(frust_material::baseline);
        let design_lang = theme.design_language;
        let effective_dark = theme.brightness == Brightness::Dark;
        let brightness = theme.brightness;

        let mut children: Vec<AnyView<AppearanceState>> = Vec::new();
        children.push(any(text("Appearance").size(28.0)));
        children.push(any(text(
            "Brightness, accent, and text size swap the whole app live.",
        )
        .size(13.0)));

        // --- Brightness + design language (the original two axes) ------------
        children.push(section_label(format!(
            "Design language: {}",
            design_choice.label()
        )));
        children.push(design_selector(design_lang, design_choice, theme.clone()));

        children.push(section_label(format!(
            "Brightness: {}",
            brightness_choice.label()
        )));
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

        // --- Accent palette --------------------------------------------------
        children.push(section_label(format!("Accent: {}", accent_choice.label())));
        children.push(accent_picker(accent_choice, brightness, theme.clone()));

        // --- Dynamic type ----------------------------------------------------
        children.push(section_label(format!(
            "Text size: {:.0}%",
            (type_factor * 100.0).round()
        )));
        children.push(type_slider(type_factor, theme.clone()));
        // Live preview scaled by the current factor.
        children.push(any(
            text("Aa — the quick brown fox").size(18.0 * type_factor)
        ));

        let content = any(FlexView::new(
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
        .cross_axis(CrossAxisAlignment::Stretch));

        // The fade veil (top `Stack` layer): a translucent surface wash painted
        // over this screen while a swap is in flight, inert (zero-size) otherwise.
        any(Stack(vec![
            content,
            veil_layer(veil_opacity, theme.scheme().surface),
        ]))
    }
}

/// A section heading text.
fn section_label(label: String) -> AnyView<AppearanceState> {
    any(Padding(
        EdgeInsets::symmetric(0.0, 4.0),
        text(label).size(16.0),
    ))
}

/// The language selector: a Material `button_group`, or a row of Cupertino
/// capsule buttons (there is no Cupertino connected-group widget).
fn design_selector(
    design_lang: DesignLanguage,
    selected: DesignChoice,
    current: Theme,
) -> AnyView<AppearanceState> {
    match design_lang {
        // Glyph and Material3 share the Material `button_group` picker chrome:
        // the Glyph catalog has no connected segmented-selector widget yet, and
        // `button_group`'s shape (labelled slots + a `selected` index) is a fine
        // fit for a settings toggle regardless of the active design language —
        // this row is settings-screen chrome, not in-app Glyph surface.
        DesignLanguage::Material3 | DesignLanguage::Glyph => {
            any(button_group::<AppearanceState, _>(
                DesignChoice::ALL.into_iter().map(|c| c.label()),
                selected.index(),
                move |st: &mut AppearanceState, idx: usize| {
                    st.controller.design.set(DesignChoice::from_index(idx));
                    spawn_apply(&st.controller, current.clone());
                },
            ))
        }
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
        _ => {
            // external design systems (DesignLanguage::Custom) fall back to Material chrome here
            any(button_group::<AppearanceState, _>(
                DesignChoice::ALL.into_iter().map(|c| c.label()),
                selected.index(),
                move |st: &mut AppearanceState, idx: usize| {
                    st.controller.design.set(DesignChoice::from_index(idx));
                    spawn_apply(&st.controller, current.clone());
                },
            ))
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
        // Glyph reuses the Material `Switch` for this toggle — the Glyph
        // catalog has no dedicated switch widget yet, and this settings-screen
        // control isn't itself in-app Glyph chrome.
        DesignLanguage::Material3 | DesignLanguage::Glyph => any(Row(vec![
            any(text("Dark").size(14.0)),
            any(SizedBox(Some(8.0), None)),
            any(Switch(effective_dark, on_toggle)),
        ])),
        DesignLanguage::Cupertino => any(Row(vec![
            any(text("Dark").size(14.0)),
            any(SizedBox(Some(8.0), None)),
            any(cupertino_switch(effective_dark, on_toggle)),
        ])),
        _ => {
            // external design systems (DesignLanguage::Custom) fall back to Material chrome here
            any(Row(vec![
                any(text("Dark").size(14.0)),
                any(SizedBox(Some(8.0), None)),
                any(Switch(effective_dark, on_toggle)),
            ]))
        }
    }
}

/// The accent picker: three swatch cards, each a solid-`primary` tile + label,
/// with a selected-ring (outlined variant) on the active one.
fn accent_picker(
    selected: AccentChoice,
    brightness: Brightness,
    current: Theme,
) -> AnyView<AppearanceState> {
    let mut cards: Vec<AnyView<AppearanceState>> = Vec::new();
    for (i, accent) in AccentChoice::SWATCHES.into_iter().enumerate() {
        if i > 0 {
            cards.push(any(SizedBox(Some(10.0), None)));
        }
        cards.push(swatch(
            accent,
            accent == selected,
            brightness,
            current.clone(),
        ));
    }
    any(FlexView::new(
        Axis::Horizontal,
        cards.into_iter().map(inflexible).collect::<Vec<_>>(),
    )
    .cross_axis(CrossAxisAlignment::Center))
}

/// One accent swatch card.
fn swatch(
    accent: AccentChoice,
    selected: bool,
    brightness: Brightness,
    current: Theme,
) -> AnyView<AppearanceState> {
    let color = accent.primary(brightness);
    let tile =
        any(SizedBox(Some(40.0), Some(40.0)).child(Image(solid_source(color)).fit(ImageFit::Fill)));
    let inner = Padding(
        EdgeInsets::all(8.0),
        Column(vec![
            any(Align(Alignment::CENTER, tile)),
            any(SizedBox(None, Some(4.0))),
            any(text(accent.label()).size(11.0)),
        ]),
    );
    // Outlined = the selected-ring; filled = unselected.
    let variant = if selected {
        CardVariant::Outlined
    } else {
        CardVariant::Filled
    };
    any(
        card(variant, inner).on_press(move |st: &mut AppearanceState| {
            st.controller.accent.set(accent);
            spawn_apply(&st.controller, current.clone());
        }),
    )
}

/// The dynamic-type slider (maps `0.0..=1.0` ↔ the type multiplier).
fn type_slider(type_factor: f32, current: Theme) -> AnyView<AppearanceState> {
    let value = type_factor_to_slider(type_factor);
    any(slider(value, move |st: &mut AppearanceState, v: f64| {
        st.controller.type_factor.set(slider_to_type_factor(v));
        spawn_apply(&st.controller, current.clone());
    }))
}

/// The fade-veil overlay layer: a full-bleed translucent `surface` wash while
/// `opacity > 0`, an inert zero-size spacer otherwise. Full-bleed via a
/// fill-to-max `SizedBox` (the incoming max clamps the requested span) wrapping a
/// stretched 1×1 image — the facade-only way to paint an arbitrary translucent
/// rectangle.
fn veil_layer(opacity: f64, surface: Color) -> AnyView<AppearanceState> {
    if opacity <= 0.001 {
        return any(SizedBox(None, None));
    }
    any(SizedBox(Some(f64::INFINITY), Some(f64::INFINITY))
        .child(Image(solid_source_alpha(surface, opacity)).fit(ImageFit::Fill)))
}

/// Trigger the fade veil, then drive `controller.apply(current)` on the UI-thread
/// local task queue. `apply` runs the `SetTheme` use case, whose
/// `set_app_theme`/`clear_app_theme` the shell picks up on its next
/// once-per-frame override poll.
fn spawn_apply(controller: &Arc<SettingsController>, current: Theme) {
    controller.trigger_veil();
    let handle = Arc::clone(controller);
    frust::spawn_local(async move {
        handle.apply(current).await;
    });
}
