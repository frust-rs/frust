//! Motion · Controls: the catalog's value controls — [`switch`], [`checkbox`],
//! [`radio`], [`input`], all five [`range_slider`] designs, and the
//! [`wheel_picker`] drum.
//!
//! Every control in beUI is **controlled**: it reports the value it wants and
//! paints whatever the next rebuild feeds back. This page is therefore the
//! clearest example of why each Motion page hosts a [`frust::component`] of its
//! own (see `crate::pages::motion::text`'s module docs) — the component's
//! [`State`] is the durable value every callback here writes into.
//!
//! Sample data is upstream's own preview copy: the notification toggle, the
//! terms/updates checkboxes, the four-plan radio group, the email/password
//! form, the five slider readouts, and the month/day/year date drum.

use frust::{
    AnyView, Column, Component, CrossAxisAlignment, Row, SizedBox, View, any, component, text,
};
use frust_beui::components::button::{ButtonSize, ButtonTone, button};
use frust_beui::components::checkbox::checkbox;
use frust_beui::components::input::input;
use frust_beui::components::radio::radio;
use frust_beui::components::range_slider::{RangeSliderVariant, range_slider};
use frust_beui::components::switch::switch;
use frust_beui::components::wheel_picker::{WheelPickerOption, wheel_picker};

use crate::AppState;
use crate::nav::{caption, heading};

/// The radio group's plans — upstream's own four, the last one disabled.
const PLANS: [(&str, &str); 4] = [
    ("starter", "Starter \u{2014} free"),
    ("pro", "Pro \u{2014} $12/mo"),
    ("team", "Team \u{2014} $29/mo"),
    ("legacy", "Legacy plan"),
];

/// The wheel picker's months.
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// This page's retained values.
pub struct State {
    switch_on: bool,
    switch_off: bool,
    terms: bool,
    updates: bool,
    plan: String,
    email: String,
    password: String,
    show_password: bool,
    query: String,
    slider_default: f64,
    slider_bubble: f64,
    slider_fluid: f64,
    slider_ruler: f64,
    slider_wave: f64,
    month: String,
    day: String,
    year: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            switch_on: true,
            switch_off: false,
            terms: true,
            updates: false,
            plan: "pro".to_string(),
            email: String::new(),
            password: "hunter2".to_string(),
            show_password: false,
            query: "Ada".to_string(),
            slider_default: 40.0,
            slider_bubble: 28.0,
            slider_fluid: 35.0,
            slider_ruler: 72.5,
            slider_wave: 45.0,
            month: "June".to_string(),
            day: "9".to_string(),
            year: "2004".to_string(),
        }
    }
}

/// A vertical spacer.
fn gap(height: f64) -> AnyView<State> {
    any(SizedBox(None, Some(height)))
}

/// A horizontal spacer.
fn hgap(width: f64) -> AnyView<State> {
    any(SizedBox(Some(width), None))
}

/// One component's block: its name, a one-line note (where a ported
/// degradation is stated), and the live instances.
fn demo(title: &str, note: &str, body: Vec<AnyView<State>>) -> AnyView<State> {
    let mut children = vec![
        any(text(title.to_string()).size(16.0)),
        gap(4.0),
        any(caption(note.to_string())),
        gap(12.0),
    ];
    children.extend(body);
    children.push(gap(32.0));
    any(Column(children).cross_axis(CrossAxisAlignment::Start))
}

/// A control beside the visible text upstream renders as a sibling element —
/// `switch`, `checkbox` and `radio` all paint no label of their own.
fn labelled(control: AnyView<State>, label: &str) -> AnyView<State> {
    any(Row(vec![
        control,
        hgap(10.0),
        any(text(label.to_string()).size(14.0)),
    ])
    .cross_axis(CrossAxisAlignment::Center))
}

/// A slider row: the control at a fixed width under its own readout.
fn slider_row(label: &str, readout: String, control: AnyView<State>) -> AnyView<State> {
    any(Column(vec![
        any(Row(vec![
            any(text(label.to_string()).size(14.0)),
            hgap(12.0),
            any(caption(readout)),
        ])
        .cross_axis(CrossAxisAlignment::Center)),
        gap(8.0),
        any(SizedBox(Some(320.0), None).child(control)),
    ])
    .cross_axis(CrossAxisAlignment::Start))
}

/// How many days that month has, so the day drum never strands its value past
/// the end of a short month — upstream's `daysIn` plus its clamping effect.
fn days_in(month: &str, year: i32) -> usize {
    match month {
        "February" => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        "April" | "June" | "September" | "November" => 30,
        _ => 31,
    }
}

/// [`switch`], [`checkbox`] and [`radio`] — the three boolean/selection rings.
fn toggles(state: &State) -> AnyView<State> {
    let switches = any(Column(vec![
        labelled(
            any(switch(state.switch_on, |s: &mut State, v| s.switch_on = v)
                .label("Enable notifications")),
            "Enable notifications",
        ),
        gap(12.0),
        labelled(
            any(switch(state.switch_off, |s: &mut State, v| s.switch_off = v).label("Off")),
            "Off",
        ),
        gap(12.0),
        labelled(
            any(switch(true, |_: &mut State, _| {})
                .disabled(true)
                .label("Disabled")),
            "Disabled \u{b7} the refusal shake",
        ),
    ])
    .cross_axis(CrossAxisAlignment::Start));

    let checkboxes = any(Column(vec![
        labelled(
            any(checkbox(state.terms, |s: &mut State, v| s.terms = v)
                .label("Accept terms and conditions")),
            "Accept terms and conditions",
        ),
        gap(12.0),
        labelled(
            any(checkbox(state.updates, |s: &mut State, v| s.updates = v)
                .label("Email me product updates")),
            "Email me product updates",
        ),
        gap(12.0),
        labelled(
            any(checkbox(true, |_: &mut State, _| {})
                .indeterminate(true)
                .label("Select all (partial)")),
            "Select all (partial)",
        ),
        gap(12.0),
        labelled(
            any(checkbox(true, |_: &mut State, _| {})
                .disabled(true)
                .label("Disabled")),
            "Disabled",
        ),
    ])
    .cross_axis(CrossAxisAlignment::Start));

    let mut radios: Vec<AnyView<State>> = Vec::new();
    for (value, label) in PLANS {
        if !radios.is_empty() {
            radios.push(gap(12.0));
        }
        let selected = state.plan == value;
        let disabled = value == "legacy";
        radios.push(labelled(
            any(radio(selected, move |s: &mut State| {
                s.plan = value.to_string();
            })
            .disabled(disabled)
            .label(label)),
            label,
        ));
    }
    let radios = any(Column(radios).cross_axis(CrossAxisAlignment::Start));

    demo(
        "switch \u{b7} checkbox \u{b7} radio",
        "All three paint no text of their own \u{2014} upstream renders the label as \
         a sibling element and so does this page, naming the control for \
         assistive tech through .label(). The checkbox's exit blur is dropped \
         (the mark still collapses on opacity and scale), and the radio dot \
         pops in place rather than gliding between items: there is no \
         shared-layout registry spanning sibling widgets here.",
        vec![any(Row(vec![
            switches,
            hgap(40.0),
            checkboxes,
            hgap(40.0),
            radios,
        ])
        .cross_axis(CrossAxisAlignment::Start))],
    )
}

/// [`input`]: label, placeholder, validation, password and success states.
fn inputs(state: &State) -> AnyView<State> {
    let email_invalid = !state.email.is_empty() && !state.email.contains('@');
    let mut email = input(state.email.clone(), |s: &mut State, v| s.email = v)
        .label("Email")
        .placeholder("you@example.com")
        .reserve_message_line(true);
    if email_invalid {
        email = email.error("Enter a valid email address.");
    }

    let password = input(state.password.clone(), |s: &mut State, v| s.password = v)
        .label("Password")
        .password(!state.show_password)
        .reserve_message_line(true);

    let query = input(state.query.clone(), |s: &mut State, v| s.query = v)
        .label("Search")
        .placeholder("Search people")
        .success(!state.query.is_empty())
        .reserve_message_line(true);

    let disabled = input("Read only", |_: &mut State, _| {})
        .label("Disabled")
        .disabled(true);

    let field = |view: AnyView<State>| any(SizedBox(Some(240.0), None).child(view));

    demo(
        "input",
        "The label is a static row above the field \u{2014} upstream's does not float \
         either, despite the reputation. Type a name without an @ to raise the \
         error row (its enter/exit keeps the rise, drops the blur); the search \
         field shows the success check. No leading/trailing icon slots are \
         ported, and inner padding is symmetric, so the check sits in the \
         trailing padding rather than in reserved space.",
        vec![
            any(
                Row(vec![field(any(email)), hgap(24.0), field(any(password))])
                    .cross_axis(CrossAxisAlignment::Start),
            ),
            gap(16.0),
            any(
                Row(vec![field(any(query)), hgap(24.0), field(any(disabled))])
                    .cross_axis(CrossAxisAlignment::Start),
            ),
            gap(12.0),
            any(button(
                if state.show_password {
                    "Hide password"
                } else {
                    "Show password"
                },
                |s: &mut State| s.show_password = !s.show_password,
            )
            .tone(ButtonTone::Outline)
            .size(ButtonSize::Sm)),
        ],
    )
}

/// All five [`range_slider`] designs.
fn sliders(state: &State) -> AnyView<State> {
    let default = slider_row(
        "Default \u{b7} tick dots, stretching thumb",
        format!("{:.0}", state.slider_default),
        any(range_slider(state.slider_default, |s: &mut State, v| {
            s.slider_default = v
        })
        .step(5.0)
        .show_ticks(true)
        .label("Value")),
    );

    let bubble = slider_row(
        "Bubble \u{b7} drag fast and it leans",
        format!("{:.0}", state.slider_bubble),
        any(
            range_slider(state.slider_bubble, |s: &mut State, v| s.slider_bubble = v)
                .variant(RangeSliderVariant::Bubble)
                .label("Value"),
        ),
    );

    let fluid = slider_row(
        "Fluid \u{b7} the pill is the control",
        format!("{:.0}%", state.slider_fluid),
        any(
            range_slider(state.slider_fluid, |s: &mut State, v| s.slider_fluid = v)
                .variant(RangeSliderVariant::Fluid)
                .label("Brightness"),
        ),
    );

    let ruler = slider_row(
        "Ruler \u{b7} fling it and it snaps to a tick",
        format!("{:.1} kg", state.slider_ruler),
        any(
            range_slider(state.slider_ruler, |s: &mut State, v| s.slider_ruler = v)
                .variant(RangeSliderVariant::Ruler)
                .range(40.0, 120.0)
                .step(0.5)
                .ruler(12.0, 10)
                .unit("kg")
                .label("Weight"),
        ),
    );

    let wave = slider_row(
        "Wave \u{b7} bars crest around the handle",
        format!("{:.0}", state.slider_wave),
        any(
            range_slider(state.slider_wave, |s: &mut State, v| s.slider_wave = v)
                .variant(RangeSliderVariant::Wave)
                .bars(32)
                .label("Gain"),
        ),
    );

    let disabled = slider_row(
        "Disabled",
        "60".to_string(),
        any(range_slider(60.0, |_: &mut State, _| {})
            .disabled(true)
            .label("Disabled")),
    );

    demo(
        "range_slider",
        "Five designs from five upstream files. The ruler's edge fade is \
         dropped (ticks hard-clip instead of fading) and its momentum is one \
         projected-and-snapped spring rather than Motion's inertia \
         integrator; the wave's bars carry velocity rather than being ramped; \
         velocity is sampled per painted frame, so the bubble's lean and the \
         ruler's fling are frame-quantised. PageUp/PageDown are not bound \
         \u{2014} the framework's key set has no page keys.",
        vec![any(Row(vec![
            any(Column(vec![default, gap(24.0), bubble, gap(24.0), fluid])
                .cross_axis(CrossAxisAlignment::Start)),
            hgap(56.0),
            any(Column(vec![ruler, gap(24.0), wave, gap(24.0), disabled])
                .cross_axis(CrossAxisAlignment::Start)),
        ])
        .cross_axis(CrossAxisAlignment::Start))],
    )
}

/// The [`wheel_picker`] drum, as upstream's date picker.
fn wheels(state: &State) -> AnyView<State> {
    let year: i32 = state.year.parse().unwrap_or(2004);
    let day_count = days_in(&state.month, year);
    let days: Vec<WheelPickerOption> = (1..=day_count)
        .map(|d| WheelPickerOption::from(d.to_string().as_str()))
        .collect();
    let months: Vec<WheelPickerOption> =
        MONTHS.iter().map(|m| WheelPickerOption::from(*m)).collect();
    let years: Vec<WheelPickerOption> = (1980..2040)
        .map(|y| WheelPickerOption::from(y.to_string().as_str()))
        .collect();

    let month_wheel = any(SizedBox(Some(150.0), None).child(
        wheel_picker(months, state.month.clone(), |s: &mut State, v| {
            s.month = v;
            // A short month can strand the day past its end — pull it back,
            // exactly as upstream's clamping effect does.
            let year: i32 = s.year.parse().unwrap_or(2004);
            let count = days_in(&s.month, year);
            if s.day.parse::<usize>().unwrap_or(1) > count {
                s.day = count.to_string();
            }
        })
        .label("Month"),
    ));

    let day_wheel = any(SizedBox(Some(90.0), None)
        .child(wheel_picker(days, state.day.clone(), |s: &mut State, v| s.day = v).label("Day")));

    let year_wheel = any(SizedBox(Some(110.0), None).child(
        wheel_picker(years, state.year.clone(), |s: &mut State, v| {
            s.year = v;
            let year: i32 = s.year.parse().unwrap_or(2004);
            let count = days_in(&s.month, year);
            if s.day.parse::<usize>().unwrap_or(1) > count {
                s.day = count.to_string();
            }
        })
        .label("Year"),
    ));

    let disabled_wheel = any(SizedBox(Some(150.0), None).child(
        wheel_picker(
            MONTHS.iter().map(|m| WheelPickerOption::from(*m)).collect(),
            "March",
            |_: &mut State, _| {},
        )
        .disabled(true)
        .visible_count(5)
        .label("Disabled"),
    ));

    demo(
        "wheel_picker",
        "A CSS 3D drum, re-derived as a 2D affine per row: the true cylinder \
         offset, the same perspective divide, and the row squash cos \u{3c8} gives. \
         Rows are not keystoned and glyphs inside one do not fan; the edge \
         fade is evaluated per row rather than per pixel; the tick sound is \
         not ported (there is no audio seam). A fling reports its landing \
         value at release \u{2014} the glide after it is purely visual.",
        vec![
            any(Row(vec![
                month_wheel,
                hgap(12.0),
                day_wheel,
                hgap(12.0),
                year_wheel,
                hgap(48.0),
                disabled_wheel,
            ])
            .cross_axis(CrossAxisAlignment::Center)),
            gap(12.0),
            any(caption(format!(
                "{} {}, {}",
                state.month, state.day, state.year
            ))),
        ],
    )
}

/// The page's interactive body.
struct ControlsPage;

impl Component for ControlsPage {
    type State = State;

    fn init(&self) -> State {
        State::default()
    }

    fn build(&self, state: &mut State) -> impl View<State> {
        any(Column(vec![
            toggles(state),
            inputs(state),
            sliders(state),
            wheels(state),
        ])
        .cross_axis(CrossAxisAlignment::Start))
    }
}

pub fn page() -> AnyView<AppState> {
    any(Column(vec![
        any(heading("Motion \u{b7} Controls")),
        any(SizedBox(None, Some(8.0))),
        any(caption(
            "Value controls: the toggle, the box, the ring, the text field, \
             all five slider designs, and the wheel drum \u{2014} every one \
             controlled, every one live.",
        )),
        any(SizedBox(None, Some(24.0))),
        any(component(ControlsPage)),
    ])
    .cross_axis(CrossAxisAlignment::Start))
}

#[cfg(test)]
mod tests {
    use super::days_in;

    /// The day drum's clamp is this page's own logic (upstream keeps it in an
    /// effect), so the month lengths it reads are pinned here.
    #[test]
    fn february_follows_the_gregorian_leap_rule() {
        assert_eq!(days_in("February", 2003), 28);
        assert_eq!(days_in("February", 2004), 29);
        assert_eq!(days_in("February", 1900), 28);
        assert_eq!(days_in("February", 2000), 29);
    }

    #[test]
    fn the_short_months_are_thirty_days() {
        for month in ["April", "June", "September", "November"] {
            assert_eq!(days_in(month, 2004), 30, "{month}");
        }
        assert_eq!(days_in("January", 2004), 31);
        assert_eq!(days_in("December", 2004), 31);
    }
}
