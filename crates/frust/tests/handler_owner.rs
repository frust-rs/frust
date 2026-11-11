//! Handlers run under the component's owner, so the timers and scoped futures they start
//! follow the component's lifetime: a rebuild leaves them running, and removing the component
//! cancels them even when the timer handle was detached. A child owner held in state cancels
//! only the timers armed under it.
//!
//! Driven through the public facade: a `Component` with a `GestureDetector` is mounted in a
//! `Tester`, tapped, then observed through counters its own callbacks bump. `Tester::pump`
//! advances a virtual clock, so the timers fire at their virtual deadlines.
//!
//! Needs the facade's `test-support` feature (`cargo test -p frust-ui --features test-support`);
//! without it the file compiles to nothing.

#![cfg(feature = "test-support")]

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use frust::testing::{Tester, find};
use frust::time::{Timer, sleep};
use frust::{
    AnyView, Component, GestureDetector, Owner, View, any, component, spawn_local_scoped, text,
};
use kurbo::Size;

const SIZE: Size = Size::new(400.0, 300.0);
const FRAME: Duration = Duration::from_millis(16);
const TAP_DELAY: Duration = Duration::from_millis(100);
const CHILD_DELAY: Duration = Duration::from_millis(300);
const SETTLE: Duration = Duration::from_secs(1);

/// Shared counters the component's callbacks bump. Cloned into the component, so the test
/// reads them after the tree has run.
#[derive(Clone, Default)]
struct Probes {
    builds: Rc<Cell<u32>>,
    tap_timer: Rc<Cell<u32>>,
    tap_future: Rc<Cell<u32>>,
    child_timer: Rc<Cell<u32>>,
}

/// Root state: whether the debounce component is mounted.
#[derive(Default)]
struct Host {
    shown: bool,
}

struct Debounce {
    probes: Probes,
}

#[derive(Default)]
struct DebounceState {
    taps: u32,
    /// The one child owner the taps reuse. Each tap cleans it up, which cancels the previous
    /// tap's child timer, before arming its own.
    child: Option<Owner>,
}

impl Component for Debounce {
    type State = DebounceState;

    fn init(&self) -> DebounceState {
        DebounceState::default()
    }

    fn build(&self, _state: &mut DebounceState) -> impl View<DebounceState> {
        self.probes.builds.set(self.probes.builds.get() + 1);
        let probes = self.probes.clone();
        let on_tap = move |s: &mut DebounceState| {
            s.taps += 1;

            // Detached: only the owner can cancel it, so the removal test proves the binding.
            let fired = probes.tap_timer.clone();
            Timer::new(TAP_DELAY, move || fired.set(fired.get() + 1)).detach();

            let done = probes.tap_future.clone();
            spawn_local_scoped(async move {
                sleep(TAP_DELAY).await;
                done.set(done.get() + 1);
            });

            let owner = s.child.get_or_insert_with(|| {
                Owner::current()
                    .expect("a handler runs under the component's owner")
                    .child()
            });
            owner.cleanup();
            let fired = probes.child_timer.clone();
            owner.with(|| {
                Timer::new(CHILD_DELAY, move || fired.set(fired.get() + 1)).detach();
            });
        };
        GestureDetector::<DebounceState, _>(text("tap me")).on_tap(on_tap)
    }
}

fn mount(probes: &Probes) -> Tester<Host, AnyView<Host>> {
    let probes = probes.clone();
    Tester::new(
        Host { shown: true },
        move |host: &mut Host| -> AnyView<Host> {
            if host.shown {
                any(component(Debounce {
                    probes: probes.clone(),
                }))
            } else {
                any(text("gone"))
            }
        },
        SIZE,
    )
}

#[test]
fn tap_timer_and_scoped_future_survive_a_rebuild() {
    let probes = Probes::default();
    let mut tester = mount(&probes);
    let builds_before_tap = probes.builds.get();

    tester.tap(find::text("tap me"));
    tester.pump(FRAME);
    assert!(
        probes.builds.get() > builds_before_tap,
        "the tap must rebuild the component so the test covers a rebuild"
    );
    assert_eq!(probes.tap_timer.get(), 0, "nothing fires before its delay");

    tester.pump(SETTLE);
    assert_eq!(probes.tap_timer.get(), 1, "the handler's timer fires once");
    assert_eq!(
        probes.tap_future.get(),
        1,
        "the handler's scoped future completes"
    );
}

#[test]
fn removing_the_component_cancels_its_pending_timer_and_future() {
    let probes = Probes::default();
    let mut tester = mount(&probes);

    tester.tap(find::text("tap me"));
    tester.pump(FRAME);
    assert_eq!(
        probes.tap_timer.get(),
        0,
        "still pending when the component is removed"
    );

    tester.state_mut().shown = false;
    tester.pump(FRAME);
    tester.pump(SETTLE);

    assert_eq!(
        probes.tap_timer.get(),
        0,
        "owner cleanup cancels the detached timer"
    );
    assert_eq!(
        probes.tap_future.get(),
        0,
        "owner cleanup aborts the scoped future"
    );
    assert_eq!(
        probes.child_timer.get(),
        0,
        "the child timer goes with the component"
    );
}

#[test]
fn cleaning_a_child_owner_cancels_only_its_timer() {
    let probes = Probes::default();
    let mut tester = mount(&probes);

    tester.tap(find::text("tap me"));
    tester.pump(FRAME);
    // The second tap cleans up the first tap's child owner before arming its own.
    tester.tap(find::text("tap me"));
    tester.pump(FRAME);
    tester.pump(SETTLE);

    assert_eq!(
        probes.child_timer.get(),
        1,
        "only the latest child timer fires; the cleaned-up one is cancelled"
    );
    assert_eq!(
        probes.tap_timer.get(),
        2,
        "the component stays alive through the cleanup"
    );
    assert_eq!(
        probes.tap_future.get(),
        2,
        "the component's scoped futures are unaffected"
    );
}
