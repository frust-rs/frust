//! Desktop entry point for the notes demo (spec §5, §12.9).
//!
//! Opens the notes screen in the preview window; the shared `AppState`/
//! `app_logic` live in the crate's library root so the interaction test can
//! drive them headlessly.

use notes::{AppState, app_logic};

fn main() {
    forgekit::App::new(AppState::new(), app_logic)
        .run()
        .unwrap();
}
