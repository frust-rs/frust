//! S8 — Plugin-call overhead (shared_preferences write+read loops).
//!
//! **Stub** — the registry, deps (`frust-shared-preferences` is already in
//! this package's `Cargo.toml`), and driver contract are wired by task 02.
//! Task 9E-04 replaces [`S8::build`]'s placeholder with the write+read loops
//! (N unique keys × all five value types, per-op latency logged as parseable
//! lines + total wall) and the burst-during-animation variant. Edit only this
//! file. Respect PROTOCOL's S8 fairness rules (unique keys for reads; report
//! writes and reads separately). Desktop uses the plugin's macOS
//! NSUserDefaults backend — fine for the smoke; measured runs are on-device.

use frust::AnyView;

use super::{BenchState, Scenario, placeholder};

/// S8 — Plugin-call overhead.
pub struct S8;

impl Scenario for S8 {
    fn id(&self) -> &'static str {
        "s8"
    }

    fn title(&self) -> &'static str {
        "Plugin-call overhead (shared_preferences)"
    }

    fn build(&self, _state: &mut BenchState) -> AnyView<BenchState> {
        placeholder(self)
    }
}
