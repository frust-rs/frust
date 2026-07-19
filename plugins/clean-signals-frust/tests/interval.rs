//! Headless tests for [`use_interval`]: it fires repeatedly while the owning
//! component is mounted, and stops (no further ticks) once the component is
//! torn down.
//!
//! Same harness as `tests/glue.rs`/`tests/async_view.rs`
//! ([`support::setup`]/[`support::frame`]/[`support::pump_until`]/
//! [`support::pump_frames`]) — a headless [`RenderRoot`] driven directly, no
//! GPU. `use_interval`'s own timer is a real [`clean_signals::time::sleep`],
//! so these tests use the same pump-poll deadline-loop pattern the async
//! tests do — never a bare `std::thread::sleep` on its own (see
//! `support::pump_until`'s doc comment for the one sanctioned sleep in this
//! harness).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use clean_signals_frust::use_interval;
use frust::{Axis, Component, FlexView, any, component, keyed, text};
use frust_core::{AnyView, RenderRoot};
use frust_text::TextContext;

mod support;
use support::{frame, pump_frames, pump_until, setup};

/// A component that ticks a shared counter every `period` via
/// [`use_interval`] for as long as it is mounted.
struct TickingScreen {
    hits: Arc<AtomicU32>,
    period: Duration,
}

impl Component for TickingScreen {
    type State = ();

    fn init(&self) {
        let hits = Arc::clone(&self.hits);
        use_interval(self.period, move || {
            hits.fetch_add(1, Ordering::SeqCst);
        });
    }

    fn build(&self, _state: &mut ()) -> AnyView<()> {
        any(text("ticking screen"))
    }
}

/// Test harness state: whether the ticking component is mounted.
struct Harness {
    show: bool,
}

fn mount_logic(
    hits: Arc<AtomicU32>,
    period: Duration,
) -> impl FnMut(&mut Harness) -> FlexView<Harness> {
    move |h: &mut Harness| -> FlexView<Harness> {
        let mut children = Vec::new();
        if h.show {
            children.push(keyed(
                1u64,
                component(TickingScreen {
                    hits: hits.clone(),
                    period,
                }),
            ));
        }
        FlexView::new(Axis::Vertical, children)
    }
}

#[test]
fn fires_at_least_n_times_while_mounted() {
    let _ambient = setup();

    let hits = Arc::new(AtomicU32::new(0));
    let period = Duration::from_millis(2);
    const N: u32 = 3;

    let mut logic = mount_logic(hits.clone(), period);
    let mut root: RenderRoot<Harness, FlexView<Harness>> = RenderRoot::new();
    let mut state = Harness { show: true };
    let mut tcx = TextContext::new();

    let first = frame(&mut root, &mut logic, &mut state, &mut tcx);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "use_interval's first tick fires after the first period, not immediately"
    );

    pump_until(&mut root, &mut logic, &mut state, &mut tcx, first, || {
        hits.load(Ordering::SeqCst) >= N
    });

    assert!(
        hits.load(Ordering::SeqCst) >= N,
        "the interval fired at least {N} times while the component stayed mounted"
    );
}

#[test]
fn stops_ticking_after_keyed_removal() {
    let _ambient = setup();

    let hits = Arc::new(AtomicU32::new(0));
    let period = Duration::from_millis(2);
    const N: u32 = 2;

    let mut logic = mount_logic(hits.clone(), period);
    let mut root: RenderRoot<Harness, FlexView<Harness>> = RenderRoot::new();
    let mut state = Harness { show: true };
    let mut tcx = TextContext::new();

    // Mount and let a couple of ticks land.
    let first = frame(&mut root, &mut logic, &mut state, &mut tcx);
    pump_until(&mut root, &mut logic, &mut state, &mut tcx, first, || {
        hits.load(Ordering::SeqCst) >= N
    });

    // Remove the keyed child: teardown → owner cleanup → use_interval's
    // on_cleanup → the alive-flag flips false, so the in-flight sleep's next
    // wakeup breaks the loop instead of ticking again.
    state.show = false;
    frame(&mut root, &mut logic, &mut state, &mut tcx);

    // Give the in-flight sleep plenty of periods to have woken up and
    // observed the cleared flag, then assert the tick count has gone flat.
    pump_frames(&mut root, &mut logic, &mut state, &mut tcx, 50);
    let after_removal = hits.load(Ordering::SeqCst);

    pump_frames(&mut root, &mut logic, &mut state, &mut tcx, 50);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        after_removal,
        "tick count stays flat across further pumped frames after removal"
    );
}
