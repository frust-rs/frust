//! S2 — Long-list scroll (10k rows, scripted fling + steady scroll).
//!
//! **Stub** — the registry, deps, and driver contract are wired by task 02.
//! Task 9E-03 replaces [`S2::build`]'s placeholder with the real 10k-row
//! virtualized `ListView` (text + generated thumbnail + icons) driven by a
//! deterministic scripted-scroll timeline. Edit only this file.

use frust::AnyView;

use super::{BenchState, Scenario, placeholder};

/// S2 — Long-list scroll.
pub struct S2;

impl Scenario for S2 {
    fn id(&self) -> &'static str {
        "s2"
    }

    fn title(&self) -> &'static str {
        "Long-list scroll (10k rows)"
    }

    fn build(&self, _state: &mut BenchState) -> AnyView<BenchState> {
        placeholder(self)
    }
}
