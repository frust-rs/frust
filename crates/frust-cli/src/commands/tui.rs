//! `frust tui` — launches the TUI workbench (Phase 1 wiring).
//!
//! The TUI provides an advanced interactive interface for managing Frust
//! projects. It handles project detection and supervision in later phases;
//! for now it shows a welcome screen outside a project and the workbench
//! inside one (see frust-tui's Phase 1 scope in PLAN.md D2).

use anyhow::Result;

/// Launches the TUI workbench. Returns 0 on success, 1 on error.
/// The TUI handles its own cwd forwarding and terminal lifecycle.
pub fn run() -> Result<u8> {
    // frust_tui::run() is async, so we need a tokio runtime to execute it.
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(frust_tui::run())?;
    Ok(0)
}
