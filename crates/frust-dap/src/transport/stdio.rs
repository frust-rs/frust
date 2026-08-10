//! # Stdio transport
//!
//! One DAP session over the process's own stdin/stdout, for the case an editor
//! spawns the adapter as a child process and talks to it through pipes.
//!
//! **Stdout is the protocol channel here.** A single stray byte written to it
//! from anywhere else desynchronises the client's framing, which is why this
//! crate is print-free and routes every diagnostic through `log` — the host
//! process decides where that goes (stderr, a file), never stdout.

use tokio::io::BufWriter;
use tokio_util::sync::CancellationToken;

use crate::protocol::codec::CodecError;
use crate::server::session::{DapAdapter, EventSender, run_session};

/// Serve exactly one DAP session over stdin/stdout and return when it ends.
///
/// `make_adapter` is called once with this session's [`EventSender`]. The
/// caller (the `frust` CLI) exits once this returns — the client has hung up,
/// asked to disconnect, or never spoke DAP at all.
///
/// `cancel` covers the bare-terminal case: an editor that closes the pipe is
/// seen as EOF, but a `frust dap` run by hand and stopped with Ctrl-C has no
/// EOF to fall back on, so the same signal-driven token tears its session down.
pub async fn run_stdio_session<A, F>(
    make_adapter: F,
    cancel: CancellationToken,
) -> Result<(), CodecError>
where
    A: DapAdapter,
    F: FnOnce(EventSender) -> A,
{
    log::debug!("frust-dap serving one session over stdio");

    // Buffered stdout still flushes per message: the codec's writer flushes
    // every frame before returning, so buffering only saves syscalls.
    let result = run_session(
        tokio::io::stdin(),
        BufWriter::new(tokio::io::stdout()),
        make_adapter,
        cancel,
    )
    .await;

    match &result {
        Ok(()) => log::debug!("frust-dap stdio session ended"),
        Err(error) => log::warn!("frust-dap stdio session ended with an error: {error}"),
    }

    result
}
