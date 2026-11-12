//! A vertical scroll view keeps the focused text caret in view: a tap on a field peeking in at
//! the bottom edge, a viewport shrunk under a visible field and keyboard traversal onto a field
//! below the fold all scroll the field inside, a user drag away from it is never undone, and
//! `reveal_focused_caret(false)` turns the whole rule off. A drag that starts on a caption beside
//! the focused field keeps the field typing; a tap there blurs it.
//!
//! Needs the facade's `test-support` feature (`cargo test -p frust-ui --features test-support`);
//! without it the file compiles to nothing.

#![cfg(feature = "test-support")]

use std::time::Duration;

use frust::testing::Tester;
use frust::{ScrollController, ScrollView, SizedBox, column, scroll_view, text, text_input};
use frust_core::{ImeEvent, InputEvent, Key, KeyEvent, Modifiers, NamedKey, View};
use kurbo::{Point, Rect, Size, Vec2};

const W: f64 = 400.0;
const FRAME: Duration = Duration::from_millis(16);

#[derive(Default)]
struct Form {
    value: String,
}

/// A `W`-wide page: an `above`-tall spacer, the field, a `below`-tall spacer.
fn page(
    above: f64,
    below: f64,
    reveal: bool,
    controller: &ScrollController,
) -> impl FnMut(&mut Form) -> ScrollView<Form> + 'static {
    let controller = controller.clone();
    move |s: &mut Form| {
        scroll_view(
            column()
                .child(SizedBox(Some(W), Some(above)))
                .child(text_input(s.value.clone(), |s: &mut Form, v: String| {
                    s.value = v
                }))
                .child(SizedBox(Some(W), Some(below))),
        )
        .controller(controller.clone())
        .reveal_focused_caret(reveal)
    }
}

/// A focused field's caret blinks forever, so settle by frame count, not by idleness.
fn frames<V: View<Form>>(tester: &mut Tester<Form, V>, count: usize) {
    for _ in 0..count {
        tester.pump(FRAME);
    }
}

/// The field's window-space box, from the retained tree.
fn field<V: View<Form>>(tester: &Tester<Form, V>) -> Rect {
    tester
        .inspect()
        .into_iter()
        .find(|node| node.type_name.ends_with("TextInputWidget"))
        .expect("the page holds a text field")
        .bounds
}

fn inside(rect: Rect, top: f64, bottom: f64) -> bool {
    rect.y0 >= top && rect.y1 <= bottom
}

#[test]
fn tapping_a_field_peeking_in_at_the_bottom_scrolls_it_inside() {
    let controller = ScrollController::new();
    let mut tester = Tester::new(
        Form::default(),
        page(280.0, 0.0, true, &controller),
        Size::new(W, 300.0),
    );
    assert!(
        !inside(field(&tester), 0.0, 300.0),
        "the field starts clipped"
    );

    tester.tap(Point::new(50.0, 290.0));
    frames(&mut tester, 6);

    assert!(controller.offset() > 0.0, "the page scrolled");
    assert_eq!(
        controller.offset(),
        controller.max_offset(),
        "clamped to its range"
    );
    assert!(
        inside(field(&tester), 0.0, 300.0),
        "field {:?}",
        field(&tester)
    );
}

#[test]
fn tabbing_onto_a_field_below_the_fold_reveals_it() {
    let controller = ScrollController::new();
    let mut tester = Tester::new(
        Form::default(),
        page(600.0, 600.0, true, &controller),
        Size::new(W, 300.0),
    );
    tester.send(&InputEvent::Key(KeyEvent {
        key: Key::Named(NamedKey::Tab),
        modifiers: Modifiers::default(),
        repeat: false,
    }));
    frames(&mut tester, 6);

    let field = field(&tester);
    assert!(inside(field, 0.0, 300.0), "field {field:?}");
    assert!(
        controller.offset() < 600.0,
        "a minimal move, not a top alignment"
    );
}

#[test]
fn a_shorter_window_reveals_the_field_it_would_cover() {
    let controller = ScrollController::new();
    let mut tester = Tester::new(
        Form::default(),
        page(200.0, 400.0, true, &controller),
        Size::new(W, 300.0),
    );
    tester.tap(Point::new(50.0, 210.0));
    frames(&mut tester, 6);
    assert_eq!(controller.offset(), 0.0, "already comfortably inside");

    tester.resize(Size::new(W, 180.0));
    frames(&mut tester, 6);
    assert!(controller.offset() > 0.0, "the shrink scrolled the page");
    assert!(
        inside(field(&tester), 0.0, 180.0),
        "field {:?}",
        field(&tester)
    );
}

#[test]
fn dragging_a_focused_field_out_of_view_is_not_undone() {
    let controller = ScrollController::new();
    let mut tester = Tester::new(
        Form::default(),
        page(250.0, 1000.0, true, &controller),
        Size::new(W, 300.0),
    );
    tester.tap(Point::new(50.0, 265.0));
    frames(&mut tester, 6);
    let resting = controller.offset();

    // Grab the field and carry it far past the top, then hold still before letting go.
    let mut drag = tester.start_gesture(Point::new(50.0, 265.0));
    for step in 1..=20 {
        tester.move_to(&mut drag, Point::new(50.0, 265.0 - 30.0 * f64::from(step)));
        tester.pump(FRAME);
    }
    for _ in 0..12 {
        tester.move_to(&mut drag, Point::new(50.0, -335.0));
        tester.pump(FRAME);
    }
    tester.up(drag);
    frames(&mut tester, 6);
    let dragged = controller.offset();
    assert!(dragged > resting + 400.0, "the drag scrolled: {dragged}");
    assert!(
        !inside(field(&tester), 0.0, 300.0),
        "the field left the view"
    );

    frames(&mut tester, 30);
    assert_eq!(controller.offset(), dragged, "the user's scroll stands");
}

#[test]
fn reveal_focused_caret_false_leaves_the_page_where_the_user_put_it() {
    let controller = ScrollController::new();
    let mut tester = Tester::new(
        Form::default(),
        page(280.0, 200.0, false, &controller),
        Size::new(W, 300.0),
    );
    tester.tap(Point::new(50.0, 290.0));
    frames(&mut tester, 6);
    assert_eq!(controller.offset(), 0.0, "no tap reveal");

    tester.resize(Size::new(W, 200.0));
    frames(&mut tester, 6);
    assert_eq!(controller.offset(), 0.0, "no resize reveal");
}

/// A caption over the field, and a tall spacer so a drag from the caption scrolls the page.
fn captioned(s: &mut Form) -> ScrollView<Form> {
    scroll_view(
        column()
            .child(SizedBox(Some(W), Some(40.0)).child(text("Caption")))
            .child(text_input(s.value.clone(), |s: &mut Form, v: String| {
                s.value = v
            }))
            .child(SizedBox(Some(W), Some(1000.0))),
    )
}

fn commit<V: View<Form>>(tester: &mut Tester<Form, V>, text: &str) {
    tester.send(&InputEvent::Ime(ImeEvent::Commit(text.to_string())));
    tester.pump(FRAME);
}

/// Focus the field with a tap and type into it.
fn focused_and_typing() -> Tester<Form, ScrollView<Form>> {
    let mut tester = Tester::new(Form::default(), captioned, Size::new(W, 300.0));
    let at = field(&tester).center();
    let finger = tester.start_touch(0, at);
    tester.up(finger);
    tester.pump(FRAME);
    commit(&mut tester, "a");
    assert_eq!(tester.state().value, "a", "the tapped field types");
    assert!(tester.semantics().focus.is_some(), "and reports focus");
    tester
}

#[test]
fn a_drag_from_a_caption_keeps_the_focused_field_typing() {
    let mut tester = focused_and_typing();
    let mut finger = tester.start_touch(0, Point::new(10.0, 20.0));
    for _ in 0..4 {
        tester.move_by(&mut finger, Vec2::new(0.0, -10.0));
        tester.pump(FRAME);
    }
    tester.up(finger);
    tester.pump(FRAME);

    commit(&mut tester, "bc");
    assert_eq!(
        tester.state().value,
        "abc",
        "the drag left the field typing"
    );
    assert!(
        tester.semantics().focus.is_some(),
        "the field still reports focus"
    );
}

#[test]
fn a_tap_on_a_caption_blurs_the_focused_field() {
    let mut tester = focused_and_typing();
    let finger = tester.start_touch(0, Point::new(10.0, 20.0));
    tester.up(finger);
    tester.pump(FRAME);

    commit(&mut tester, "bc");
    assert_eq!(tester.state().value, "a", "a blurred field takes no text");
    assert!(tester.semantics().focus.is_none(), "nothing reports focus");
}
