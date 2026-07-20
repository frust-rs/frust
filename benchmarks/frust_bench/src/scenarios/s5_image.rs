//! S5 — Image pipeline (decode-and-display stream while scrolling).
//!
//! **Stub** — the registry, deps, and driver contract are wired by task 02.
//! Task 9E-03 replaces [`S5::build`]'s placeholder with a stream of large
//! images decoded off-thread via `frust::decode_image_async` + `use_task`
//! while scrolling, images generated deterministically in memory (no committed
//! assets). Edit only this file.

use frust::AnyView;

use super::{BenchState, Scenario, placeholder};

/// S5 — Image pipeline.
pub struct S5;

impl Scenario for S5 {
    fn id(&self) -> &'static str {
        "s5"
    }

    fn title(&self) -> &'static str {
        "Image pipeline (off-thread decode stream)"
    }

    fn build(&self, _state: &mut BenchState) -> AnyView<BenchState> {
        placeholder(self)
    }
}
