//! Transport selection for the DAP server.
//!
//! Two transports are supported, and they are mutually exclusive:
//!
//! - [`TransportMode::Stdio`] — exactly one session over the process's own
//!   stdin/stdout, the shape an editor uses when it spawns the adapter as a
//!   child process (VS Code's `debugAdapterExecutable`, Zed, Helix, nvim-dap).
//!   In this mode **stdout is the protocol channel**: nothing else may write
//!   to it (see the crate-root doc).
//! - [`TransportMode::Tcp`] — a loopback listener serving one session per
//!   accepted connection, for editors configured with `debugServer` and for
//!   scripted testing.
//!
//! The listener implementation lives in [`crate::server`]; [`stdio`] holds the
//! single-session stdin/stdout entry point.

pub mod stdio;

/// How the DAP server accepts its client(s).
///
/// The TCP variant carries only a port: the bind address is **not**
/// configurable — see [`crate::server::serve_tcp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMode {
    /// Serve exactly one session over stdin/stdout and return when it ends.
    Stdio,

    /// Listen on `127.0.0.1:<port>` and serve one session per connection.
    ///
    /// `port: 0` asks the OS for an ephemeral port; the resolved port is
    /// logged and reported through [`crate::server::serve_tcp`]'s `ready`
    /// channel.
    Tcp {
        /// TCP port to bind on loopback. `0` = OS-assigned.
        port: u16,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_mode_tcp_carries_only_a_port() {
        let mode = TransportMode::Tcp { port: 4849 };
        match mode {
            TransportMode::Tcp { port } => assert_eq!(port, 4849),
            TransportMode::Stdio => panic!("expected Tcp"),
        }
    }

    #[test]
    fn test_transport_mode_tcp_port_zero_is_valid() {
        assert!(matches!(
            TransportMode::Tcp { port: 0 },
            TransportMode::Tcp { port: 0 }
        ));
    }

    #[test]
    fn test_transport_mode_is_copy_and_eq() {
        let mode = TransportMode::Stdio;
        let copied = mode;
        assert_eq!(mode, copied);
        assert_ne!(TransportMode::Stdio, TransportMode::Tcp { port: 0 });
    }
}
