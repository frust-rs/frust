//! The embedded DAP server's handle and status — the exact counterpart of
//! [`McpServerHandle`](super::McpServerHandle)/[`McpStatus`](super::McpStatus),
//! deliberately shaped the same way.
//!
//! The two embedded servers are the same problem twice: a live resource the
//! pure `update` cannot construct, started and stopped from
//! `crate::engine::Engine`, reporting its bound port and its ending back as
//! ordinary [`Message`](crate::engine::Message)s that carry a **generation**
//! tag. Keeping the shapes identical is what makes the second one cost a
//! reader nothing: everything
//! `docs/TUI_ARCHITECTURE.md`'s Embedded MCP Surface says about generations,
//! late reports, and a `CancellationToken` that does not cancel on drop is
//! true here word for word.
//!
//! What differs is only what each server *is*: the DAP one serves editors
//! over loopback TCP (`frust_dap::serve_embedded`) rather than agents over
//! Streamable HTTP, and its client registry is `frust_dap`'s.

use frust_dap::DapClientRegistry;
use tokio_util::sync::CancellationToken;

/// The embedded DAP server the workbench is currently running.
///
/// Held on [`AppState::dap`](crate::engine::AppState::dap) but created and
/// destroyed only by [`Engine::start_dap`](crate::engine::Engine::start_dap)/
/// `stop_dap`. Cloning the model clones the handle: both clones name the same
/// server.
#[derive(Clone)]
pub struct DapServerHandle {
    /// Which server instance this is (`Engine`'s monotonic counter) — the tag
    /// both of this server's reports carry, so a superseded server's late
    /// report can be told apart from the running one's. See
    /// [`McpServerHandle::generation`](super::McpServerHandle::generation) for
    /// why an ungated late report is not merely untidy but leaves a listener
    /// nothing can stop.
    generation: u64,
    /// What stops the server (`frust_dap::serve_embedded`'s shutdown seam).
    cancel: CancellationToken,
    /// The connected-client registry the server registers each DAP connection
    /// in — the clients display's live source.
    registry: DapClientRegistry,
    /// `None` until the listener reports the port it actually bound (the only
    /// way to learn an OS-assigned port from `bind_port = 0`).
    bound_port: Option<u16>,
}

impl DapServerHandle {
    /// A handle for a server that has been spawned but has not reported its
    /// bound port yet, tagged with the `generation`
    /// [`Engine::start_dap`](crate::engine::Engine::start_dap) minted for it.
    pub(crate) fn starting(
        generation: u64,
        cancel: CancellationToken,
        registry: DapClientRegistry,
    ) -> Self {
        Self {
            generation,
            cancel,
            registry,
            bound_port: None,
        }
    }

    /// Which server instance this handle names.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Record the port the listener actually bound. Returns whether this
    /// changed anything (so the caller knows whether to redraw).
    pub(crate) fn set_bound_port(&mut self, port: u16) -> bool {
        let changed = self.bound_port != Some(port);
        self.bound_port = Some(port);
        changed
    }

    /// Ask the server to shut down. Idempotent.
    pub(crate) fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Every editor attached to this server right now, oldest first. Read
    /// through [`AppState::dap_clients`](crate::engine::AppState::dap_clients)
    /// rather than off the handle.
    pub fn clients(&self) -> Vec<frust_dap::DapClientEntry> {
        self.registry.snapshot()
    }

    /// What the status display shows right now.
    pub fn status(&self) -> DapStatus {
        match self.bound_port {
            Some(port) => DapStatus::Listening {
                port,
                clients: self.registry.count(),
            },
            None => DapStatus::Starting,
        }
    }
}

impl std::fmt::Debug for DapServerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `DapClientRegistry` is not `Debug`; the live count is the useful part.
        f.debug_struct("DapServerHandle")
            .field("generation", &self.generation)
            .field("bound_port", &self.bound_port)
            .field("clients", &self.registry.count())
            .finish()
    }
}

/// What the workbench's embedded DAP server is doing — the whole surface the
/// UI needs (and the only one it should read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DapStatus {
    /// No server is running.
    Stopped,
    /// Spawned; the listener has not reported its bound port yet.
    Starting,
    /// Listening on `127.0.0.1:port` with `clients` editors attached.
    Listening {
        /// The bound loopback port.
        port: u16,
        /// How many DAP clients are attached right now.
        clients: usize,
    },
}
