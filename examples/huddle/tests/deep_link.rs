//! Deep-link routing smoke for the Huddle skeleton (task 10).
//!
//! `push_deep_link` writes a process-wide signal, so this is deliberately the
//! ONE test in its own binary that touches it (mirroring `examples/navdemo`'s
//! single-test-per-binary convention) — it can't race a sibling test over that
//! shared state.

use forgekit::{AnyView, Component};
use forgekit_core::RenderRoot;

use huddle::{HuddleApp, HuddleState};

mod support;
use support::{serial, setup};

type Root = RenderRoot<HuddleState, AnyView<HuddleState>>;

#[test]
fn warm_deep_link_to_a_parameterized_route_resolves_and_builds() {
    let _g = serial();
    let _ambient = setup();

    let mut root: Root = RenderRoot::new();
    let mut state = HuddleApp.init();
    let mut logic = |s: &mut HuddleState| HuddleApp.build(s);

    root.rebuild(&mut logic, &mut state);
    assert!(root.root_id().is_some(), "the shell builds at \"/\"");

    // A warm deep link (the desktop dev seam) reaches a user id no visible list
    // button leads to, exercising `/user/:id` param capture end-to-end.
    forgekit::push_deep_link("/user/7");
    root.rebuild(&mut logic, &mut state);
    assert!(
        root.root_id().is_some(),
        "a deep link to an id-parameterized route must resolve and build"
    );
}
