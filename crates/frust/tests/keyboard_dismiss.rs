//! A vertical scroll view can opt in to dismissing the soft keyboard on a downward drag
//! (`dismiss_keyboard_on_drag(true)`): the focus session ends and the page still scrolls. Off (the
//! default), the same drag keeps the focused field typing, and an upward drag never dismisses.
//!
//! Needs the facade's `test-support` feature (`cargo test -p frust-ui --features test-support`);
//! without it the file compiles to nothing.

#![cfg(feature = "test-support")]

use std::time::Duration;

use frust::testing::Tester;
use frust::{ScrollView, SizedBox, column, scroll_view, text, text_input};
use frust_core::{ImeEvent, InputEvent, View};
use kurbo::{Point, Size, Vec2};

const W: f64 = 400.0;
const FRAME: Duration = Duration::from_millis(16);

#[derive(Default)]
struct Form {
    value: String,
    dismiss: bool,
}

/// A caption over the field, and a tall spacer so a drag from the caption scrolls the page.
fn page(s: &mut Form) -> ScrollView<Form> {
    scroll_view(
        column()
            .child(SizedBox(Some(W), Some(40.0)).child(text("Caption")))
            .child(text_input(s.value.clone(), |s: &mut Form, v: String| {
                s.value = v
            }))
            .child(SizedBox(Some(W), Some(1000.0))),
    )
    .dismiss_keyboard_on_drag(s.dismiss)
}

fn commit<V: View<Form>>(tester: &mut Tester<Form, V>, text: &str) {
    tester.send(&InputEvent::Ime(ImeEvent::Commit(text.to_string())));
    tester.pump(FRAME);
}

/// Focus the field with a tap and type into it.
fn focused_and_typing(dismiss: bool) -> Tester<Form, ScrollView<Form>> {
    let state = Form {
        dismiss,
        ..Form::default()
    };
    let mut tester = Tester::new(state, page, Size::new(W, 300.0));
    let at = tester
        .inspect()
        .into_iter()
        .find(|node| node.type_name.ends_with("TextInputWidget"))
        .expect("the page holds a text field")
        .bounds
        .center();
    let finger = tester.start_touch(0, at);
    tester.up(finger);
    tester.pump(FRAME);
    commit(&mut tester, "a");
    assert_eq!(tester.state().value, "a", "the tapped field types");
    assert!(tester.semantics().focus.is_some(), "and reports focus");
    tester
}

/// Drag from the caption by 4 steps of `dy`, then lift.
fn drag(tester: &mut Tester<Form, ScrollView<Form>>, dy: f64) {
    let mut finger = tester.start_touch(0, Point::new(10.0, 20.0));
    for _ in 0..4 {
        tester.move_by(&mut finger, Vec2::new(0.0, dy));
        tester.pump(FRAME);
    }
    tester.up(finger);
    tester.pump(FRAME);
}

#[test]
fn a_downward_drag_dismisses_the_keyboard_when_opted_in() {
    let mut tester = focused_and_typing(true);
    drag(&mut tester, 10.0);

    assert!(tester.semantics().focus.is_none(), "the session ended");
    commit(&mut tester, "bc");
    assert_eq!(tester.state().value, "a", "a dismissed field takes no text");
}

#[test]
fn a_downward_drag_keeps_the_field_typing_by_default() {
    let mut tester = focused_and_typing(false);
    drag(&mut tester, 10.0);

    commit(&mut tester, "bc");
    assert_eq!(
        tester.state().value,
        "abc",
        "the drag left the field typing"
    );
    assert!(tester.semantics().focus.is_some());
}

#[test]
fn an_upward_drag_keeps_the_field_typing_even_when_opted_in() {
    let mut tester = focused_and_typing(true);
    drag(&mut tester, -10.0);

    commit(&mut tester, "bc");
    assert_eq!(
        tester.state().value,
        "abc",
        "an upward drag never dismisses"
    );
    assert!(tester.semantics().focus.is_some());
}
