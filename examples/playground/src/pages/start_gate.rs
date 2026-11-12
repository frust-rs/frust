//! Gate section: a `frust::StartGate` over a task that waits about two seconds.
//!
//! The page hosts one gate. Its task sleeps for two seconds and then either succeeds with a run
//! number and the time it took, or fails with a message, according to the **Fail next run**
//! checkbox as it stood when the task started. **Run again** remounts the gate so the task runs
//! again: the gate sits in one of two arms of an `Either`, chosen by the parity of a run counter
//! held in the page state, so bumping the counter swaps arms and mounts a fresh gate.
//!
//! The splash, error and ready views are `View<()>` and cannot read the page state, which is why
//! the ready view shows only what the task produced.

use std::fmt;
use std::time::Instant;

use frust::time::{Duration, sleep};
use frust::{
    ButtonStyle, Color, EdgeInsets, Either, Get, Padding, RwSignal, Set, SizedBox, StartGate,
    Theme, Update, View, WithUntracked, button, checkbox, column, text, use_context,
};

use crate::PlaygroundState;

/// How long the start-up task waits before it reports.
const WAIT: Duration = Duration::from_secs(2);

/// Horizontal inset of page content.
const SIDE: f64 = 16.0;

/// The "Gate" section's state, owned by [`PlaygroundState`].
#[derive(Clone)]
pub struct GateState {
    /// How many times the gate has been mounted; its parity picks the `Either` arm.
    pub run: RwSignal<u32>,
    /// Whether the next run fails.
    pub fail_next: RwSignal<bool>,
}

impl GateState {
    /// Fresh state: the first run, set to succeed.
    pub fn new() -> Self {
        GateState {
            run: RwSignal::new(0),
            fail_next: RwSignal::new(false),
        }
    }
}

impl Default for GateState {
    fn default() -> Self {
        Self::new()
    }
}

/// The start-up task's failure.
#[derive(Debug)]
struct Refused(u32);

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Start-up failed on run {} (Fail next run was on)",
            self.0
        )
    }
}

impl std::error::Error for Refused {}

/// What a successful run produced.
struct Produced {
    run: u32,
    millis: u128,
}

fn theme() -> Theme {
    use_context::<Theme>().unwrap_or_else(frust_material::baseline)
}

fn accent() -> Color {
    theme().scheme().primary
}

fn muted() -> Color {
    theme().scheme().on_surface_variant
}

/// Inset a content item from the screen edges.
fn inset<V: View<PlaygroundState>>(view: V) -> impl View<PlaygroundState> {
    Padding(EdgeInsets::symmetric(SIDE, 2.0), view)
}

/// A muted caption line.
fn caption(content: &str) -> impl View<PlaygroundState> + use<> {
    inset(text(content.to_string()).size(11.0).color(muted()))
}

/// A gate whose task waits [`WAIT`] and then follows the toggle as it stood at mount.
fn gate(state: &GateState) -> StartGate<Produced> {
    let fail_next = state.fail_next;
    let run = state.run.with_untracked(|run| *run);
    StartGate::new(move || {
        // Read on the UI thread when the task starts, so the future captures plain values.
        let fail = fail_next.with_untracked(|fail| *fail);
        async move {
            let started = Instant::now();
            sleep(WAIT).await;
            if fail {
                Err(Refused(run))
            } else {
                Ok(Produced {
                    run,
                    millis: started.elapsed().as_millis(),
                })
            }
        }
    })
    .splash(text("Splash: starting up, about two seconds...").size(14.0))
    .ready(|produced: &Produced| {
        text(format!(
            "Ready: run {} produced its value after {} ms",
            produced.run, produced.millis
        ))
        .size(14.0)
    })
}

/// See the page-fn contract in [`crate::pages`].
pub fn page(state: &PlaygroundState) -> impl View<PlaygroundState> {
    let gate_state = &state.start_gate;
    let run = gate_state.run.get();
    let fail_next = gate_state.fail_next.get();
    let (run_signal, fail_signal) = (gate_state.run, gate_state.fail_next);

    // Two arms with the same gate: changing the arm remounts it, which restarts the task.
    let hosted = if run.is_multiple_of(2) {
        Either::Left(gate(gate_state))
    } else {
        Either::Right(gate(gate_state))
    };

    column()
        .child(SizedBox(None, Some(8.0)))
        .child(inset(text("Gate").size(13.0).color(accent())))
        .child(inset(text(format!("run: {run}")).size(12.0)))
        .child(SizedBox(None, Some(8.0)))
        .child(inset(checkbox(
            fail_next,
            "Fail next run",
            move |_state: &mut PlaygroundState, requested: bool| fail_signal.set(requested),
        )))
        .child(caption(
            "The toggle is read when a run starts; changing it mid-run does not affect that run.",
        ))
        .child(inset(
            button("Run again", move |_state: &mut PlaygroundState| {
                run_signal.update(|count| *count += 1);
            })
            .style(ButtonStyle::Primary)
            .small(),
        ))
        .child(caption(
            "Run again remounts the gate: the splash shows again for about two seconds.",
        ))
        .child(SizedBox(None, Some(8.0)))
        .child(inset(hosted))
        .child(caption(
            "Expect the splash line for about two seconds, then the ready line with the run \
             number and the elapsed milliseconds. With Fail next run on, expect the error \
             message and a Retry button instead; Retry shows the splash again and runs the \
             task once more, reading the toggle again.",
        ))
        .child(SizedBox(None, Some(8.0)))
}
