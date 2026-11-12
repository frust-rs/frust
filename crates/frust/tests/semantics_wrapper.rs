//! A semantics wrapper turns a tappable child into one labelled accessibility element, and an
//! accessibility click on it reaches the child's tap handler.
//!
//! Needs the facade's `test-support` feature (`cargo test -p frust-ui --features test-support`);
//! without it the file compiles to nothing.

#![cfg(feature = "test-support")]

use std::cell::Cell;
use std::rc::Rc;

use frust::accesskit::{Action, Role};
use frust::testing::{Tester, find};
use frust::{GestureDetector, SizedBox, semantics};
use kurbo::Size;

#[derive(Default)]
struct Taps {
    count: u32,
}

#[test]
fn clicking_a_button_wrapper_runs_the_childs_tap_handler_once() {
    let seen = Rc::new(Cell::new(0u32));
    let probe = seen.clone();
    let mut tester = Tester::new(
        Taps::default(),
        move |_: &mut Taps| {
            let probe = probe.clone();
            semantics(
                GestureDetector::<Taps, _>(SizedBox(Some(120.0), Some(48.0))).on_tap(
                    move |s: &mut Taps| {
                        s.count += 1;
                        probe.set(probe.get() + 1);
                    },
                ),
            )
            .button()
            .label("Open")
        },
        Size::new(400.0, 300.0),
    );

    tester.perform_action(find::label(Role::Button, "Open"), Action::Click);
    assert_eq!(seen.get(), 1, "the tap handler runs exactly once");
}
