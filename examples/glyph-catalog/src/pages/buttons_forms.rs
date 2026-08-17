//! Buttons + Forms section — Reference §02 Buttons + §03 Form Controls.
//!
//! Every [`ButtonStyle`] (Primary/Secondary/Ghost/Danger/Icon), the `.small()`
//! padding scale, and the built-in press-feedback scale are demonstrated
//! alongside the baseline form controls (`text_input`, `checkbox`, `radio`,
//! `slider`) — **BASELINE widgets rendered under the Glyph theme** (they
//! aren't `frust_glyph::*` catalog components; they resolve their
//! colors/shapes from the active [`Theme`](frust::Theme) like any other
//! themed widget, which is how they read correctly here). The row's boolean
//! control is the one exception: it is `frust_glyph::toggle`, the Glyph
//! catalog's own authored toggle-spring switch (`plugins/glyph/src/toggle.rs`)
//! rather than a themed baseline widget — the row shows baseline form
//! controls plus the glyph toggle, side by side, under the Glyph theme.
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
//! timed 2s auto-reset would be preferable but needs an async sleep
//! primitive; `examples/huddle`'s toast auto-dismiss uses
//! `clean_signals::time::sleep` under `frust::spawn`, but that crate is a
//! sibling-checkout-only dependency this standalone example doesn't carry
//! (and adding one is out of scope for this page) — so this page falls back
//! to a manual toggle instead, which still exercises the same `.loading()`
//! builder path interactively.
//!
//! # Disabled state — no dedicated builder
//!
//! `ButtonView` has no `.enabled(false)`/disabled builder (checked directly
//! against `button.rs`); `.loading(true)` is the *only* seam that suppresses
//! interaction and reports disabled accessibility semantics, so it doubles as
//! the disabled-look demo below, captioned honestly rather than presented as
//! a dedicated disabled API.
//!
//! # `TextInputView::enabled(false)` / `.obscured(true)` and stretched button
//! label alignment
//!
//! [`disabled_vs_enabled_row`] pairs a `.enabled(false)` field against an
//! enabled one, both pre-filled so the dimmed-chrome contrast is visible
//! without typing (`TextInputView` — unlike `ButtonView` above — *does* carry
//! a dedicated `.enabled(..)` builder). The obscured demo pre-fills
//! `.obscured(true)` with real text so the bullet masking is visible at a
//! glance. [`stretched_button_row`] and [`vertical_alignment_row`] exercise
//! `ButtonView::label_alignment` (`button.rs`): the former shows the
//! stretch-aware *default* auto-centering a width-stretched button's label
//! with no explicit call (beside an unaffected natural-width button), the
//! latter shows that a height-stretched button's default never auto-centers
//! vertically — only an explicit `.label_alignment(Alignment::CENTER)` does.
//! No design-system catalog (`material`/`cupertino`/`glyph`) wraps
//! `TextInput`, so this page — run under the Glyph theme — is the only place
//! the baseline `TextInput`'s themed token resolution gets a design-language
//! look; Material/Cupertino disabled appearance is **not** covered here (see
//! `examples/huddle`'s four-way appearance toggle for that check).

use frust::{
    Alignment, AnyView, Axis, ButtonStyle, Column, Component, CrossAxisAlignment, EdgeInsets,
    FlexView, Get, Padding, RwSignal, Set, SizedBox, any, button, checkbox, component, inflexible,
    radio, slider, text, text_input,
};
// The Glyph catalog's own authored toggle — not a baseline `frust`/
// `frust-widgets` item, and not a themed stand-in for one either.
use frust_glyph::toggle;

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
    /// The `toggle` demo's controlled on/off state.
    toggle_on: RwSignal<bool>,
    /// The `slider` demo's controlled value (`0.0..=1.0`), live-read out as a
    /// percentage beside it.
    slider_value: RwSignal<f64>,
    /// The disabled-vs-enabled `TextInput` pair's disabled-side value
    /// (`.enabled(false)` — see the [module docs](self)). Pre-seeded and
    /// effectively frozen: a disabled field refuses focus, so nothing ever
    /// writes back to this signal.
    disabled_input_value: RwSignal<String>,
    /// The disabled-vs-enabled pair's enabled-side value, for contrast.
    enabled_compare_value: RwSignal<String>,
    /// The `.obscured(true)` demo's value, pre-filled so the bullet masking
    /// is visible without typing (see the [module docs](self)).
    obscured_value: RwSignal<String>,
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
            toggle_on: RwSignal::new(false),
            slider_value: RwSignal::new(0.4),
            disabled_input_value: RwSignal::new("Locked value".to_string()),
            enabled_compare_value: RwSignal::new("Editable value".to_string()),
            obscured_value: RwSignal::new("hunter2".to_string()),
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
        let toggle_on = state.toggle_on.get();
        let slider_value = state.slider_value.get();
        let disabled_input_value = state.disabled_input_value.get();
        let enabled_compare_value = state.enabled_compare_value.get();
        let obscured_value = state.obscured_value.get();

        let content = Column(vec![
            heading("Buttons"),
            caption(
                "Every ButtonStyle, .small(), and the 0.96 press-feedback scale (try clicking one).",
            ),
            spacer(8.0),
            button_styles_row(),
            spacer(12.0),
            caption("Small (.small()):"),
            spacer(4.0),
            small_buttons_row(),
            spacer(16.0),
            caption(
                "Loading demo — toggled by the checkbox, not by pressing the button itself: .loading(true) suppresses on_press while shown (see the module docs).",
            ),
            spacer(4.0),
            loading_demo_row(loading),
            spacer(16.0),
            caption(
                "Disabled look — ButtonView has no .enabled(false) seam; .loading(true) is the only disabled-semantics builder, so it doubles here.",
            ),
            spacer(4.0),
            disabled_demo_row(),
            spacer(16.0),
            caption(
                "Stretched label alignment (.label_alignment — task 08): the wide CTA below has no explicit call and still centers its label, because the stretch-aware default auto-centers on width-only stretch; the natural-width button beside it is unaffected either way.",
            ),
            spacer(4.0),
            stretched_button_row(),
            spacer(12.0),
            caption(
                "Height stretch never auto-centers by default (contrast the top-pinned label on the left) — only an explicit .label_alignment(Alignment::CENTER) centers both axes (right).",
            ),
            spacer(4.0),
            vertical_alignment_row(),
            spacer(24.0),
            heading("Form Controls"),
            caption(
                "BASELINE widgets (text_input/checkbox/radio/slider) rendered under the Glyph theme — not frust_glyph::* catalog components, but themed the same way. The toggle below is the exception: a frust_glyph::toggle catalog widget.",
            ),
            spacer(12.0),
            caption("Text input (prompt-style placeholder):"),
            spacer(4.0),
            any(
                text_input(input_value, |s: &mut ButtonsFormsState, v: String| {
                    s.input_value.set(v)
                })
                .placeholder("> type a command"),
            ),
            spacer(12.0),
            caption("Multiline textarea (.multiline(4)):"),
            spacer(4.0),
            any(
                text_input(textarea_value, |s: &mut ButtonsFormsState, v: String| {
                    s.textarea_value.set(v)
                })
                .placeholder("Write a longer note...")
                .multiline(4),
            ),
            spacer(12.0),
            caption(
                "Disabled vs enabled (.enabled(false) — task 07): both pre-filled so the dimmed chrome/content on the left is visible without typing; the disabled field also refuses focus.",
            ),
            spacer(4.0),
            disabled_vs_enabled_row(disabled_input_value, enabled_compare_value),
            spacer(12.0),
            caption(
                "Obscured / password mode (.obscured(true) — task 07): pre-filled with real text, masked with bullets; the underlying value is untouched.",
            ),
            spacer(4.0),
            any(
                text_input(obscured_value, |s: &mut ButtonsFormsState, v: String| {
                    s.obscured_value.set(v)
                })
                .obscured(true),
            ),
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
            caption(
                "Toggle (frust_glyph::toggle — a spring-driven knob travel plus a fading track/border color, both independently timed — toggle it):",
            ),
            spacer(4.0),
            any(toggle(toggle_on, |s: &mut ButtonsFormsState, v: bool| {
                s.toggle_on.set(v)
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
    any(FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(button("Primary", |_: &mut ButtonsFormsState| {})),
            inflexible(hspacer(8.0)),
            inflexible(
                button("Secondary", |_: &mut ButtonsFormsState| {}).style(ButtonStyle::Secondary),
            ),
            inflexible(hspacer(8.0)),
            inflexible(button("Ghost", |_: &mut ButtonsFormsState| {}).style(ButtonStyle::Ghost)),
            inflexible(hspacer(8.0)),
            inflexible(button("Danger", |_: &mut ButtonsFormsState| {}).style(ButtonStyle::Danger)),
            inflexible(hspacer(8.0)),
            inflexible(button("+", |_: &mut ButtonsFormsState| {}).style(ButtonStyle::Icon)),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center))
}

/// A `.small()` Primary + Secondary pair.
fn small_buttons_row() -> AnyView<ButtonsFormsState> {
    any(FlexView::new(
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
    .cross_axis(CrossAxisAlignment::Center))
}

/// The loading demo: a checkbox driving a Primary button's `.loading(..)`.
fn loading_demo_row(loading: bool) -> AnyView<ButtonsFormsState> {
    any(FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(checkbox(
                loading,
                "Loading",
                |s: &mut ButtonsFormsState, v: bool| s.loading.set(v),
            )),
            inflexible(hspacer(12.0)),
            inflexible(button("Save changes", |_: &mut ButtonsFormsState| {}).loading(loading)),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center))
}

/// The disabled-look demo: a Secondary button statically shown with
/// `.loading(true)` (see the [module docs](self) on why no dedicated
/// disabled builder exists).
fn disabled_demo_row() -> AnyView<ButtonsFormsState> {
    any(button("Unavailable", |_: &mut ButtonsFormsState| {})
        .style(ButtonStyle::Secondary)
        .loading(true))
}

/// Stretch-aware default from `ButtonView::label_alignment`: a
/// button forced wider than its natural content ([`SizedBox`]-wrapped)
/// auto-centers its label with *no* explicit `.label_alignment(..)` call,
/// beside a natural-width button that's unaffected — there's no free space
/// for any alignment fraction to distribute into.
fn stretched_button_row() -> AnyView<ButtonsFormsState> {
    any(FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(
                SizedBox(Some(220.0), None)
                    .child(button("Continue", |_: &mut ButtonsFormsState| {})),
            ),
            inflexible(hspacer(12.0)),
            inflexible(button("Continue", |_: &mut ButtonsFormsState| {})),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center))
}

/// Contrasts the default vs an explicit `Alignment::CENTER` on a button
/// stretched on *both* axes (`SizedBox(width, height)`, both tightened): the
/// default's y-component stays top-pinned even when x auto-centers
/// (`ButtonView::label_alignment`'s module docs — height stretch never
/// auto-centers), so an explicit call is the only way to also center
/// vertically.
fn vertical_alignment_row() -> AnyView<ButtonsFormsState> {
    any(FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(
                SizedBox(Some(160.0), Some(64.0))
                    .child(button("Default", |_: &mut ButtonsFormsState| {})),
            ),
            inflexible(hspacer(12.0)),
            inflexible(
                SizedBox(Some(160.0), Some(64.0)).child(
                    button("Centered", |_: &mut ButtonsFormsState| {})
                        .label_alignment(Alignment::CENTER),
                ),
            ),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center))
}

/// The disabled-vs-enabled `TextInput` pair (`.enabled(false)`):
/// same style, one refuses focus and dims its chrome/content, the other is
/// untouched — both pre-filled so the contrast is visible at a glance.
fn disabled_vs_enabled_row(
    disabled_value: String,
    enabled_value: String,
) -> AnyView<ButtonsFormsState> {
    any(FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(
                text_input(disabled_value, |s: &mut ButtonsFormsState, v: String| {
                    s.disabled_input_value.set(v)
                })
                .enabled(false),
            ),
            inflexible(hspacer(12.0)),
            inflexible(text_input(
                enabled_value,
                |s: &mut ButtonsFormsState, v: String| s.enabled_compare_value.set(v),
            )),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center))
}

/// Two mutually-exclusive radios, `selected` = the currently-chosen index.
fn radio_pair_row(selected: usize) -> AnyView<ButtonsFormsState> {
    any(FlexView::new(
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
    .cross_axis(CrossAxisAlignment::Center))
}

/// The slider plus its live percentage readout.
fn slider_row(value: f64) -> AnyView<ButtonsFormsState> {
    any(FlexView::new(
        Axis::Horizontal,
        vec![
            inflexible(slider(value, |s: &mut ButtonsFormsState, v: f64| {
                s.slider_value.set(v)
            })),
            inflexible(hspacer(12.0)),
            inflexible(text(format!("{:.0}%", value * 100.0)).size(13.0)),
        ],
    )
    .cross_axis(CrossAxisAlignment::Center))
}
