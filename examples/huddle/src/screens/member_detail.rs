//! `/member/:id` detail screen — REAL (shipped in task 02).
//!
//! Reached by path param (`router.push("/member/:id")`) or by a deep link (the
//! nav playground's `push_deep_link("/member/7")`). Shows the member header
//! plus an edit-name flow built on [`push_for_result`](NavigatorController::push_for_result):
//! "Edit name" pushes a rename page whose `TextInput` is controlled by the
//! shared [`ShellSignals::member_draft`] draft (so a per-frame reconcile does
//! not fight in-progress typing); saving pops the draft back as a
//! [`PopResult`], which the pusher's callback lands in the shared "last result"
//! banner. Faithful to `examples/navdemo`'s item-detail pattern, over the
//! huddle's member domain.

use forgekit::{
    AnyView, Button, Column, Get, NavigatorController, PopResult, Row, RwSignal, Set, SizedBox,
    any, text, text_input,
};

use crate::ShellSignals;
use crate::ShellState;

/// The member-detail page for `id`. `controller` drives back/edit navigation;
/// `signals` carries the rename draft and the last-result banner.
pub fn member_detail(
    id: String,
    controller: NavigatorController<ShellState>,
    signals: ShellSignals,
) -> AnyView<ShellState> {
    let member_draft = signals.member_draft;
    let last_result = signals.last_result;

    let mut children: Vec<AnyView<ShellState>> = Vec::new();
    children.push(any(text(format!("Member #{id}")).size(28.0)));
    children.push(any(text(
        "Route: /member/:id — reached by path param or a deep link.",
    )
    .size(14.0)));

    if let Some(result) = last_result.get() {
        children.push(any(text(format!("Last result: {result}")).size(16.0)));
    }

    let back_controller = controller.clone();
    let edit_controller = controller;
    let edit_id = id;
    children.push(any(Row(vec![
        any(Button("Back", move |_s: &mut ShellState| {
            back_controller.pop();
        })),
        any(SizedBox(Some(12.0), None)),
        any(Button(
            "Edit name (push for result)",
            move |_s: &mut ShellState| {
                let page_controller = edit_controller.clone();
                let edit_id = edit_id.clone();
                edit_controller.push_for_result(
                    move || {
                        member_edit_page(edit_id.clone(), member_draft, page_controller.clone())
                    },
                    move |s: &mut ShellState, result: PopResult| {
                        if let Some(name) = result.take::<String>() {
                            s.signals
                                .last_result
                                .set(Some(format!("Renamed to \"{name}\"")));
                        }
                    },
                );
            },
        )),
    ])));

    any(Column(children))
}

/// The rename sub-page opened by "Edit name": a `TextInput` bound to
/// `member_draft`, a "Save & return" that pops the draft as a result, and a
/// "Cancel" that pops empty.
fn member_edit_page(
    id: String,
    member_draft: RwSignal<String>,
    controller: NavigatorController<ShellState>,
) -> AnyView<ShellState> {
    let save_controller = controller.clone();
    let cancel_controller = controller;

    any(Column(vec![
        any(text(format!("Edit member #{id}")).size(22.0)),
        any(
            text_input(member_draft.get(), move |_s: &mut ShellState, v: String| {
                member_draft.set(v);
            })
            .placeholder("New name"),
        ),
        any(Row(vec![
            any(Button("Save & return", move |_s: &mut ShellState| {
                save_controller.pop_with_result(PopResult::of(member_draft.get()));
            })),
            any(SizedBox(Some(12.0), None)),
            any(Button("Cancel", move |_s: &mut ShellState| {
                cancel_controller.pop();
            })),
        ])),
    ]))
}
