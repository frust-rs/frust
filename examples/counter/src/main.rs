//! Desktop entry point for the counter demo (spec §5, §12.9).
//!
//! Runs the counter screen through `forgekit::run` — the desktop preview
//! `runApp` equivalent for a root `Component` (phase 5.5). The shared
//! `AppState`/`CounterApp` live in the crate's library root so the
//! interaction test can drive them headlessly.

use counter::CounterApp;

fn main() {
    forgekit::run(CounterApp).unwrap();
}
