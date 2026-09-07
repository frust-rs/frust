//! `Base`-design cases for the website's `widgets/base/input` catalog page
//! set: `checkbox`, `radio`, `slider`, `text-input`. See the crate docs and
//! `crate::base` for the pure-`View`/slug-rule contract every case in this
//! registry follows, and [`super::framed`] for why none of these cases fills
//! its own backdrop.

use frust_core::AnyView;
use frust_widgets::{
    Axis, CrossAxisAlignment, FlexView, SizedBox, checkbox, inflexible, radio, slider, text,
    text_input,
};

use super::framed;
use crate::case::{Case, Design};

fn checkbox_case() -> AnyView<()> {
    framed(
        FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(checkbox(true, "Notifications", |_: &mut (), _: bool| {})),
                inflexible(SizedBox(Some(28.0), None)),
                inflexible(checkbox(
                    false,
                    "Marketing emails",
                    |_: &mut (), _: bool| {},
                )),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    )
}

fn radio_case() -> AnyView<()> {
    framed(
        FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(radio(true, "Option A").on_select(|_: &mut ()| {})),
                inflexible(SizedBox(Some(28.0), None)),
                inflexible(radio(false, "Option B").on_select(|_: &mut ()| {})),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    )
}

fn slider_case() -> AnyView<()> {
    framed(
        FlexView::new(
            Axis::Horizontal,
            vec![
                inflexible(slider(0.4, |_: &mut (), _: f64| {})),
                inflexible(SizedBox(Some(12.0), None)),
                inflexible(text("40%").size(13.0)),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    )
}

fn text_input_case() -> AnyView<()> {
    framed(
        FlexView::new(
            Axis::Vertical,
            vec![
                inflexible(SizedBox(Some(280.0), None).child(
                    text_input("", |_: &mut (), _: String| {}).placeholder("Write a note..."),
                )),
                inflexible(SizedBox(None, Some(16.0))),
                inflexible(
                    SizedBox(Some(280.0), None)
                        .child(text_input("Frust rocks", |_: &mut (), _: String| {})),
                ),
            ],
        )
        .cross_axis(CrossAxisAlignment::Center),
    )
}

/// This module's slice of the registry [`crate::cases`] concatenates.
pub const CASES: &[Case] = &[
    Case {
        slug: "checkbox",
        title: "Checkbox",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: checkbox_case,
    },
    Case {
        slug: "radio",
        title: "Radio",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: radio_case,
    },
    Case {
        slug: "slider",
        title: "Slider",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: slider_case,
    },
    Case {
        slug: "text-input",
        title: "Text Input",
        size: Case::DEFAULT_SIZE,
        scale: Case::DEFAULT_SCALE,
        time_ms: Case::DEFAULT_TIME_MS,
        design: Design::Base,
        build: text_input_case,
    },
];
