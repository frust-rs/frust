//! # The service entry point
//!
//! [`run_blocking`] is the whole public surface a front-end needs: hand it a
//! [`DapConfig`] and it runs a DAP server to completion, returning the process
//! exit code. The runtime is built **here**, not by the caller, so `frust-cli`
//! stays sync and thin — the same hand-off shape `frust tui` uses.
//!
//! **Stdout discipline.** In stdio mode stdout *is* the DAP wire. Nothing in
//! this crate writes to it except the codec, and nothing in it prints at all
//! (`log::` only); where a front-end sends its log output is the front-end's
//! decision, and in stdio mode that decision must not be stdout.

use std::sync::Arc;
use std::time::Duration;

use crate::adapter::{OrchestrationAdapter, Runner};
use crate::server::{ServerError, serve};
use crate::transport::TransportMode;

/// Exit code for a server that ran and ended cleanly.
const EXIT_OK: u8 = 0;

/// How long runtime teardown waits for background work after the server ends.
///
/// A launched session's log pump reads its subscription on a blocking task,
/// and dropping a runtime waits for blocking tasks to finish. The pumps notice
/// their cancel flag within one tick, so this is only a backstop — but an
/// unbounded wait here would turn any straggling task into a hung process.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// How a DAP server run is configured.
///
/// Deliberately two fields: which pipe to speak over, and the one
/// `ProcessRunner` every session shells out through. Everything else about a
/// run (the project, the device, the build mode) arrives per connection, in a
/// `launch` request.
pub struct DapConfig {
    /// Stdio (one session) or loopback TCP (one session per connection).
    pub mode: TransportMode,

    /// The process runner sessions shell out through — `frust-cli` builds
    /// exactly one `RealProcessRunner`; a test injects a fake.
    pub runner: Runner,
}

impl DapConfig {
    /// A config for `mode`, shelling out through `runner`.
    pub fn new(mode: TransportMode, runner: Runner) -> Self {
        Self { mode, runner }
    }
}

/// What starting or running a DAP server can fail with.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// The tokio runtime could not be built (thread or file-descriptor
    /// exhaustion) — the server never started.
    #[error("failed to build the frust-dap runtime: {0}")]
    Runtime(#[source] std::io::Error),

    /// The server itself failed — in practice, a TCP port that could not be
    /// bound.
    #[error(transparent)]
    Server(#[from] ServerError),
}

/// Run a DAP server to completion, returning the process exit code.
///
/// Builds a multi-threaded runtime internally and blocks on it. Stdio mode
/// returns when its single session ends; TCP mode serves connections until the
/// process is stopped, so it returns only on a bind failure.
pub fn run_blocking(config: DapConfig) -> Result<u8, ServiceError> {
    let DapConfig { mode, runner } = config;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("frust-dap")
        .build()
        .map_err(ServiceError::Runtime)?;

    let served = runtime.block_on(async move {
        serve(mode, move |events| {
            OrchestrationAdapter::new(events, Arc::clone(&runner))
        })
        .await
    });

    // Bounded: a blocking log-reader task that somehow outlived its session
    // must not be able to hold the process open.
    runtime.shutdown_timeout(SHUTDOWN_GRACE);

    served?;
    Ok(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::Arc;

    use frust_drive::process::FakeProcessRunner;

    use super::*;

    /// A bind failure is reported, not panicked on, and it comes back through
    /// the service's own error type rather than as a mystery exit code.
    #[test]
    fn a_taken_port_is_reported_as_a_bind_failure() {
        let squatter = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind squatter");
        let port = squatter.local_addr().expect("local addr").port();

        let result = run_blocking(DapConfig::new(
            TransportMode::Tcp { port },
            Arc::new(FakeProcessRunner::new()),
        ));

        match result {
            Err(ServiceError::Server(ServerError::Bind { port: reported, .. })) => {
                assert_eq!(reported, port);
            }
            other => panic!("expected a bind failure, got {other:?}"),
        }
    }

    #[test]
    fn a_config_keeps_the_transport_it_was_given() {
        let config = DapConfig::new(
            TransportMode::Tcp { port: 4849 },
            Arc::new(FakeProcessRunner::new()),
        );
        assert_eq!(config.mode, TransportMode::Tcp { port: 4849 });
    }
}
