//! Showcase · navigation playground — REAL (ported from `examples/navdemo` in
//! task 02).
//!
//! Exercises the nav stack end-to-end through the facade alone: a live
//! transition-preset switcher (writing [`ShellSignals::transition`], applied by
//! [`ShellApp::build`](crate::ShellApp)), a swipe-back note, a push-for-result
//! round trip landing its payload in the shared "last result" banner, and a
//! "simulate deep link" button driving the desktop dev seam
//! ([`push_deep_link`]) straight to `/member/7` — an id the roster never lists,
//! proving a deep link reaches a page no visible button leads to.

use forgekit::{
    AnyView, Button, Column, Get, NavigatorController, PopResult, Row, Set, SizedBox, any,
    push_deep_link, text,
};

use crate::ShellSignals;
use crate::ShellState;
use crate::shell::TransitionChoice;

/// The navigation playground. `controller` drives the push-for-result demo;
/// `signals` carries the live transition preset and the last-result banner.
pub fn nav_playground(
    controller: NavigatorController<ShellState>,
    signals: ShellSignals,
) -> AnyView<ShellState> {
    let transition = signals.transition;
    let last_result = signals.last_result;

    let mut children: Vec<AnyView<ShellState>> = Vec::new();
    children.push(any(text("Navigation playground").size(28.0)));
    children.push(any(text(format!(
        "Transition preset: {}",
        transition.get().label()
    ))
    .size(14.0)));

    // -- Transition-preset switcher (writes the shared signal) --------------
    children.push(any(Row(vec![
        any(Button("None", move |_s: &mut ShellState| {
            transition.set(TransitionChoice::None);
        })),
        any(SizedBox(Some(8.0), None)),
        any(Button("M3 shared-axis", move |_s: &mut ShellState| {
            transition.set(TransitionChoice::M3SharedAxis);
        })),
        any(SizedBox(Some(8.0), None)),
        any(Button("M3 fade-through", move |_s: &mut ShellState| {
            transition.set(TransitionChoice::M3FadeThrough);
        })),
        any(SizedBox(Some(8.0), None)),
        any(Button("iOS push", move |_s: &mut ShellState| {
            transition.set(TransitionChoice::Ios);
        })),
    ])));
    children.push(any(text(
        "iOS push arms the interactive left-edge swipe-back.",
    )
    .size(12.0)));

    // -- Push-for-result round trip + banner --------------------------------
    if let Some(result) = last_result.get() {
        children.push(any(text(format!("Last result: {result}")).size(16.0)));
    }
    let result_controller = controller;
    children.push(any(Button(
        "Open a page for a result",
        move |_s: &mut ShellState| {
            let inner = result_controller.clone();
            result_controller.push_for_result(
                move || result_demo_page(inner.clone()),
                move |s: &mut ShellState, result: PopResult| {
                    let message = result
                        .take::<String>()
                        .unwrap_or_else(|| "Closed with no result".to_string());
                    s.signals.last_result.set(Some(message));
                },
            );
        },
    )));

    // -- Deep-link dev seam -------------------------------------------------
    children.push(any(Button(
        "Simulate deep link \u{2192} /member/7",
        |_s: &mut ShellState| {
            push_deep_link("/member/7");
        },
    )));

    any(Column(children))
}

/// The transient page the push-for-result demo opens: two buttons, one popping
/// a result payload, one popping empty.
fn result_demo_page(controller: NavigatorController<ShellState>) -> AnyView<ShellState> {
    let return_controller = controller.clone();
    let back_controller = controller;
    any(Column(vec![
        any(text("Pick a result to return").size(22.0)),
        any(text("Route pushed via push_for_result — its payload lands in the banner.").size(12.0)),
        any(Row(vec![
            any(Button("Return \"Alpha\"", move |_s: &mut ShellState| {
                return_controller.pop_with_result(PopResult::of("Alpha".to_string()));
            })),
            any(SizedBox(Some(12.0), None)),
            any(Button("Back (no result)", move |_s: &mut ShellState| {
                back_controller.pop();
            })),
        ])),
    ]))
}
