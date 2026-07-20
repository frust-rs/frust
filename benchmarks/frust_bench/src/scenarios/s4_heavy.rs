//! S4 — Heavy-work responsiveness (parse ~50MB while animating).
//!
//! **Stub** — the registry, deps, and driver contract are wired by task 02.
//! Task 9E-04 replaces [`S4::build`]'s placeholder with a deterministic ~50MB
//! in-memory payload parsed via `use_task(|| async { spawn_blocking(parse) })`
//! while an S1-style animation runs, scenario markers bracketing the parse.
//! Edit only this file.

use frust::AnyView;

use super::{BenchState, Scenario, placeholder};

/// S4 — Heavy-work responsiveness.
pub struct S4;

impl Scenario for S4 {
    fn id(&self) -> &'static str {
        "s4"
    }

    fn title(&self) -> &'static str {
        "Heavy-work responsiveness (parse while animating)"
    }

    fn build(&self, _state: &mut BenchState) -> AnyView<BenchState> {
        placeholder(self)
    }
}
