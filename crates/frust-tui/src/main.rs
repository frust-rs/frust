//! The standalone `frust-tui` binary. `frust-cli` gains a `frust tui`
//! subcommand (task 04) that calls the same [`frust_tui::run`] entry.

use anyhow::Result;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    frust_tui::run().await
}
