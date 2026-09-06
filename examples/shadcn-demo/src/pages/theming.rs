//! Theming: the seven `ShadcnBase` presets plus a light/dark toggle, driven
//! live through `frust::set_app_theme`/`Theme::with_brightness` — the
//! `DesignLanguage::Custom("shadcn")` identity and the `ShadcnTokens`
//! extension round-trip proof is the running app repainting every shipped
//! component in the newly chosen palette, not a printed assertion. Also
//! carries the font-verification row: a body line in Inter, a `kbd` row in
//! JetBrains Mono.

use frust::{Brightness, Column, Row, SizedBox, View, any, set_app_theme, text};
use frust_shadcn::{ButtonVariant, ShadcnBase, button, kbd, kbd_group, separator, theme_for};

use crate::AppState;

pub struct State {
    pub base: ShadcnBase,
    pub dark: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            base: ShadcnBase::Neutral,
            dark: false,
        }
    }
}

fn apply(base: ShadcnBase, dark: bool) {
    let brightness = if dark {
        Brightness::Dark
    } else {
        Brightness::Light
    };
    set_app_theme(theme_for(base).with_brightness(brightness));
}

fn gap() -> frust::AnyView<AppState> {
    any(SizedBox(None, Some(16.0)))
}

pub fn page(state: &mut State) -> impl View<AppState> + use<> {
    let base = state.base;
    let dark = state.dark;

    let mut base_buttons: Vec<frust::AnyView<AppState>> = Vec::with_capacity(ShadcnBase::ALL.len());
    for b in ShadcnBase::ALL {
        let variant = if b == base {
            ButtonVariant::Default
        } else {
            ButtonVariant::Outline
        };
        base_buttons.push(any(button(b.id(), move |s: &mut AppState| {
            s.theming.base = b;
            apply(b, s.theming.dark);
        })
        .variant(variant)));
        base_buttons.push(any(SizedBox(Some(8.0), None)));
    }

    Column(vec![
        any(crate::nav::heading("Theming")),
        any(SizedBox(None, Some(16.0))),
        any(text(format!(
            "Active: DesignLanguage::Custom(\"shadcn\") — base = {}, brightness = {}",
            base.id(),
            if dark { "dark" } else { "light" },
        ))
        .size(14.0)),
        gap(),
        any(Row(base_buttons)),
        gap(),
        any(button(
            if dark {
                "Switch to Light"
            } else {
                "Switch to Dark"
            },
            move |s: &mut AppState| {
                s.theming.dark = !s.theming.dark;
                apply(s.theming.base, s.theming.dark);
            },
        )),
        gap(),
        any(separator()),
        gap(),
        any(text(
            "This paragraph is set in Inter — the sans stack every shadcn \
             text role resolves to (frust_shadcn::tokens::sans_family()).",
        )
        .size(16.0)
        .family(frust_shadcn::tokens::sans_family())),
        gap(),
        any(Row(vec![
            any(text("JetBrains Mono:").size(13.0)),
            any(SizedBox(Some(8.0), None)),
            any(kbd_group(vec![
                any(kbd("Ctrl")),
                any(kbd("Shift")),
                any(kbd("P")),
            ])),
        ])),
    ])
}
