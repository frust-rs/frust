//! Desktop entry point for the counter demo (spec §5, §12.9).
//!
//! Opens the counter screen in the preview window; the shared `AppState`/
//! `app_logic` live in the crate's library root so the interaction test can
//! drive them headlessly.

use counter::{AppState, app_logic};

fn main() {
    forgekit::App::new(AppState::default(), app_logic)
        .run()
        .unwrap();
}
