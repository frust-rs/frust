//! Buttons + Forms section — Reference §02 Buttons + §03 Form Controls.
//!
//! Every [`ButtonStyle`] (Primary/Secondary/Ghost/Danger/Icon), the `.small()`
//! padding scale, and the built-in press-feedback scale are demonstrated
//! alongside the baseline form controls (`text_input`, `checkbox`, `radio`,
//! `switch`, `slider`) — all **BASELINE widgets rendered under the Glyph
//! theme** (they aren't `frust::glyph::*` catalog components; they resolve
//! their colors/shapes from the active [`Theme`](frust::Theme) like any other
//! themed widget, which is how they read correctly here).
//!
//! # Local interactive state (component-nested, not `CatalogState`)
//!
//! Every control on this page is a controlled component wired to a signal, so
//! this page hosts its own local [`Component`](frust::Component)
//! ([`ButtonsFormsScreen`]/[`ButtonsFormsState`]) instead of adding fields to
//! the shared [`CatalogState`] — the page-fn contract's `state: &CatalogState`
//! argument is unused here (`_state`) since none of this page's demos need the
//! shell's brightness/toast/nav handles. `ComponentView<C>` implements
//! `View<Outer>` for **any** outer state (`frust_core::component`'s doc), so
//! `component(ButtonsFormsScreen)` embeds directly as an `AnyView<CatalogState>`
//! with a state boundary the outer tree never sees. Nesting also matters for
//! signal freshness: unlike a bare `RwSignal::new(..)` created inline in a
//! free function (which would re-seed to its initial value on every
//! shell-level rebuild — a tab switch, a brightness toggle — since the whole
//! `home_page` closure re-runs), a `Component`'s `State` is retained by its
//! `ComponentWidget` across rebuilds and only re-seeded via `init` on first
//! mount.
//!
//! # `.loading(true)` demo — honest deviation from a self-toggling press
//!
//! `ButtonView::loading(true)` suppresses its own `on_press` (see
//! `button.rs`'s module docs) as part of reporting disabled semantics while
//! shown, so a button can never toggle itself back off by pressing it a
//! second time — the loading demo is instead driven by a paired checkbox. A
//! timed 2s auto-reset (the task's stated preference) needs an async sleep
//! primitive; `examples/huddle`'s toast auto-dismiss uses
//! `clean_signals::time::sleep` under `frust::spawn`, but that crate is a
//! sibling-checkout-only dependency this standalone example doesn't carry
//! (and adding one is out of this page's `src/pages/buttons_forms.rs`-only
//! scope) — so this page falls back to the task's stated alternative, a
//! manual toggle, which still exercises the same `.loading()` builder path
//! interactively.
//!
//! # Disabled state — no dedicated builder
//!
//! `ButtonView` has no `.enabled(false)`/disabled builder (checked directly
//! against `button.rs`); `.loading(true)` is the *only* seam that suppresses
//! interaction and reports disabled accessibility semantics, so it doubles as
//! the disabled-look demo below, captioned honestly rather than presented as
//! a dedicated disabled API.

use frust::{
    AnyView, Axis, ButtonStyle, Column, Component, CrossAxisAlignment, EdgeInsets, FlexView, Get,
    Padding, RwSignal, Set, SizedBox, any, button, checkbox, component, inflexible, radio, slider,
    switch, text, text_input,
};

use crate::CatalogState;

/// See the page-fn contract in [`crate::pages`]. Unused: every demo on this
/// page lives in its own nested [`ButtonsFormsScreen`] component state (see
/// the [module docs](self)).
pub fn page(_state: &CatalogState) -> AnyView<CatalogState> {
    any(component(ButtonsFormsScreen))
}

/// Stateless configuration; all state lives in [`ButtonsFormsState`].
struct ButtonsFormsScreen;

/// Retained local state for every control on this page — one signal per
/// controlled component (see the [module docs](self) on why this lives here
/// rather than in [`CatalogState`]).
struct ButtonsFormsState {
    /// Drives the loading-demo button's `.loading(..)`; flipped by its paired
    /// checkbox (see the [module docs](self) `.loading(true)` section).
    loading: RwSignal<bool>,
    /// The prompt-style single-line `text_input`'s controlled value.
    input_value: RwSignal<String>,
    /// The multiline `text_input` (`.multiline(..)`)'s controlled value.
    textarea_value: RwSignal<String>,
    /// The `checkbox` demo's controlled checked state.
    checkbox_checked: RwSignal<bool>,
    /// The radio-pair demo's selected index (0 = Option A, 1 = Option B).
    radio_selected: RwSignal<usize>,
    /// The `switch` demo's controlled on/off state.
    switch_on: RwSignal<bool>,
    /// The `slider` demo's controlled value (`0.0..=1.0`), live-read out as a
    /// percentage beside it.
    slider_value: RwSignal<f64>,
}

impl Component for ButtonsFormsScreen {
    type State = ButtonsFormsState;

    fn init(&self) -> ButtonsFormsState {
        ButtonsFormsState {
            loading: RwSignal::new(false),
            input_value: RwSignal::new(String::new()),
            textarea_value: RwSignal::new(String::new()),
            checkbox_checked: RwSignal::new(true),
            radio_selected: RwSignal::new(0),
            switch_on: RwSignal::new(false),
            slider_value: RwSignal::new(0.4),
        }
    }

    fn build(&self, state: &mut ButtonsFormsState) -> AnyView<ButtonsFormsState> {
        // Tracked reads: a later write from any control below wakes this
        // component's own rebuild (never the outer CatalogState tree).
        let loading = state.loading.get();
        let input_value = state.input_value.get();
        let textarea_value = state.textarea_value.get();
        let checkbox_checked = state.checkbox_checked.get();
        let radio_selected = state.radio_selected.get();
        let switch_on = state.switch_on.get();
        let slider_value = state.slider_value.get();

        let content = Column(vec![
            heading("Buttons"),
            caption("Every ButtonStyle, .small(), and the 0.96 press-feedback scale (try clicking one)."),
            spacer(8.0),
            button_styles_row(),
            spacer(12.0),
            caption("Small (.small()):"),
            spacer(4.0),
            small_buttons_row(),
            spacer(16.0),
            caption("Loading demo — toggled by the checkbox, not by pressing the button itself: .loading(true) suppresses on_press while shown (see the module docs)."),
            spacer(4.0),
            loading_demo_row(loading),
            spacer(16.0),
            caption("Disabled look — ButtonView has no .enabled(false) seam; .loading(true) is the only disabled-semantics builder, so it doubles here."),
            spacer(4.0),
            disabled_demo_row(),
            spacer(24.0),
            heading("Form Controls"),
            caption("BASELINE widgets (text_input/checkbox/radio/switch/slider) rendered under the Glyph theme — not frust::glyph::* catalog components, but themed the same way."),
            spacer(12.0),
            caption("Text input (prompt-style placeholder):"),
            spacer(4.0),
            any(text_input(
                input_value,
                |s: &mut ButtonsFormsState, v: String| s.input_value.set(v),
            )
            .placeholder("> type a command")),
            spacer(12.0),
            caption("Multiline textarea (.multiline(4)):"),
            spacer(4.0),
            any(text_input(
                textarea_value,
                |s: &mut ButtonsFormsState, v: String| s.textarea_value.set(v),
            )
            .placeholder("Write a longer note...")
            .multiline(4)),
            spacer(12.0),
            any(checkbox(
                checkbox_checked,
                "Enable notifications",
                |s: &mut ButtonsFormsState, v: bool| s.checkbox_checked.set(v),
            )),
            spacer(12.0),
            caption("Radio pair:"),
            spacer(4.0),
            radio_pair_row(radio_selected),
            spacer(12.0),
            caption("Switch (spring-driven thumb travel via the theme's default_spatial spring, plus a track-color transition — toggle it):"),
            spacer(4.0),
            any(switch(switch_on, |s: &mut ButtonsFormsState, v: bool| {
                s.switch_on.set(v)
            })),
            spacer(16.0),
            caption("Slider with live value readout:"),
            spacer(4.0),
            slider_row(slider_value),
        ]);

        any(Padding(EdgeInsets::all(16.0), content))
    }
}

/// A section heading.
fn heading(label: &str) -> AnyView<ButtonsFormsState> {
    any(text(label).size(20.0))
}

/// A small explanatory caption line.
fn caption(label: &str) -> AnyView<ButtonsFormsState> {
    any(text(label).size(11.0))
}

/// A vertical spacer of `height` logical px.
fn spacer(height: f64) -> AnyView<ButtonsFormsState> {
    any(SizedBox(None, Some(height)))
}

/// A horizontal spacer of `width` logical px.
fn hspacer(width: f64) -> AnyView<ButtonsFormsState> {
    any(SizedBox(Some(width), None))
}

/// Every [`ButtonStyle`] variant, side by side (display-only, no-op presses).
fn button_styles_row() -> AnyView<ButtonsFormsState> {
    any(
        FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(button("Primary", |_: &mut ButtonsFormsState| {})),
                inflexible(hspacer(8.0)),
                inflexible(
                    button("Secondary", |_: &mut ButtonsFormsState| {})
                        .style(ButtonStyle::Secondary),
                ),
                inflexible(hspacer(8.0)),
                inflexible(
                    button("Ghost", |_: &mut ButtonsFormsState| {}).style(ButtonStyle::Ghost),
                ),
                inflexible(hspacer(8.0)),
                inflexible(
                    button("Danger", |_: &mut ButtonsFormsState| {}).style(ButtonStyle::Danger),
                ),
                inflexible(hspacer(8.0)),
                inflexible(
                    button("+", |_: &mut ButtonsFormsState| {}).style(ButtonStyle::Icon),
                ),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    )
}

/// A `.small()` Primary + Secondary pair.
fn small_buttons_row() -> AnyView<ButtonsFormsState> {
    any(
        FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(button("Small primary", |_: &mut ButtonsFormsState| {}).small()),
                inflexible(hspacer(8.0)),
                inflexible(
                    button("Small secondary", |_: &mut ButtonsFormsState| {})
                        .style(ButtonStyle::Secondary)
                        .small(),
                ),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    )
}

/// The loading demo: a checkbox driving a Primary button's `.loading(..)`.
fn loading_demo_row(loading: bool) -> AnyView<ButtonsFormsState> {
    any(
        FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(checkbox(
                    loading,
                    "Loading",
                    |s: &mut ButtonsFormsState, v: bool| s.loading.set(v),
                )),
                inflexible(hspacer(12.0)),
                inflexible(
                    button("Save changes", |_: &mut ButtonsFormsState| {}).loading(loading),
                ),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    )
}

/// The disabled-look demo: a Secondary button statically shown with
/// `.loading(true)` (see the [module docs](self) on why no dedicated
/// disabled builder exists).
fn disabled_demo_row() -> AnyView<ButtonsFormsState> {
    any(
        button("Unavailable", |_: &mut ButtonsFormsState| {})
            .style(ButtonStyle::Secondary)
            .loading(true),
    )
}

/// Two mutually-exclusive radios, `selected` = the currently-chosen index.
fn radio_pair_row(selected: usize) -> AnyView<ButtonsFormsState> {
    any(
        FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(
                    radio(selected == 0, "Option A")
                        .on_select(|s: &mut ButtonsFormsState| s.radio_selected.set(0)),
                ),
                inflexible(hspacer(16.0)),
                inflexible(
                    radio(selected == 1, "Option B")
                        .on_select(|s: &mut ButtonsFormsState| s.radio_selected.set(1)),
                ),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    )
}

/// The slider plus its live percentage readout.
fn slider_row(value: f64) -> AnyView<ButtonsFormsState> {
    any(
        FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(slider(value, |s: &mut ButtonsFormsState, v: f64| {
                    s.slider_value.set(v)
                })),
                inflexible(hspacer(12.0)),
                inflexible(text(format!("{:.0}%", value * 100.0)).size(13.0)),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    )
}
