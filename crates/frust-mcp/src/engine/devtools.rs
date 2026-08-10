//! The devtools half of a session: turn a discovery line into a connected,
//! handshook [`DevtoolsClient`], then keep the session's frame-stats ring fed.
//!
//! One thread per accepted discovery. It does the whole blocking sequence —
//! Android `adb forward`, a bounded reachability probe, connect, handshake,
//! frame-stats subscribe — and then becomes the subscription's drain loop.
//! Port of the flow `frust-tui`'s `supervise::devtools_bridge` runs, minus
//! the TEA message plumbing: here the results land directly on the session.
//!
//! **The probe is not redundant.** `DevtoolsClient::connect` uses the
//! platform's own TCP connect, which this side cannot bound; without a
//! `connect_timeout` probe first, a thread that must notice its stop flag
//! could park for as long as the platform's connect takes. Probing puts the
//! only unbounded wait under [`CONNECT_TIMEOUT`]; the client's own connect
//! that follows is to an already-proven-reachable loopback port.

use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use frust_devtools_protocol::Discovery;
use frust_drive::devtools_client::{DevtoolsClient, adb_forward_ephemeral, adb_forward_remove};

use super::Runner;
use super::session::Session;

/// Bound on the reachability probe — see the module doc.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// The connection's socket read/write timeout, and therefore each request's
/// own wait bound (`DevtoolsClient::connect`'s contract).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the frame-stats drain blocks per receive before re-checking the
/// session's stop flag — the loop's wind-down bound at teardown.
const FRAME_POLL_SLICE: Duration = Duration::from_millis(200);

/// Spawns the connect thread for `discovery`, registering it on the session
/// for teardown to join. A no-op once teardown has begun.
pub(crate) fn spawn_connect(session: &Arc<Session>, runner: &Runner, discovery: Discovery) {
    if session.stopping() {
        return;
    }
    let thread_session = Arc::clone(session);
    let thread_runner = Arc::clone(runner);
    let handle = thread::spawn(move || run(thread_session, thread_runner, discovery));
    session.add_thread(handle);
}

fn run(session: Arc<Session>, runner: Runner, discovery: Discovery) {
    match connect(&session, &runner, &discovery) {
        Ok(Some(client)) => pump_frames(&session, &client),
        // Teardown overtook the connect — nothing to report.
        Ok(None) => {}
        Err(err) => session.set_devtools_error(format!("{err:#}")),
    }
}

/// Allocates the Android forward (if any), connects, handshakes, and records
/// the connected client on the session. `Ok(None)` if teardown began first.
fn connect(
    session: &Arc<Session>,
    runner: &Runner,
    discovery: &Discovery,
) -> Result<Option<Arc<DevtoolsClient>>> {
    // Android's devtools port lives on the device's loopback, not the host's;
    // an ephemeral `adb forward` maps it to a host port. Recorded on the
    // session so teardown can remove exactly the forward it allocated.
    let local_port = match session.target.android_serial() {
        Some(serial) => {
            let local_port = adb_forward_ephemeral(runner.as_ref(), serial, discovery.port)
                .with_context(|| {
                    format!(
                        "forwarding the device's devtools port {} to the host",
                        discovery.port
                    )
                })?;
            if let Some((serial, local_port)) = session.set_forward(serial.to_string(), local_port)
            {
                // Teardown already removed whatever forwards it knew about, so
                // this one is ours to clean up: an `adb forward` nobody removes
                // outlives the session on the host for good. Best-effort — a
                // device that has gone away takes its forwards with it.
                let _ = adb_forward_remove(runner.as_ref(), &serial, local_port);
                return Ok(None);
            }
            local_port
        }
        None => discovery.port,
    };
    if session.stopping() {
        return Ok(None);
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], local_port));
    let probe = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .with_context(|| format!("connecting to the devtools service at {addr}"))?;
    drop(probe);
    if session.stopping() {
        return Ok(None);
    }

    let client = DevtoolsClient::connect(addr, REQUEST_TIMEOUT, discovery.token.as_deref())
        .with_context(|| format!("connecting to the devtools service at {addr}"))?;
    let info = client
        .handshake()
        .context("the devtools handshake was rejected")?;

    let client = Arc::new(client);
    session.set_connected(Arc::clone(&client), local_port, info);
    Ok(Some(client))
}

/// Arms the frame-stats push and drains it into the session's ring until the
/// connection ends or teardown begins.
///
/// A subscription that the server refuses is recorded as a devtools error but
/// does **not** un-connect the session: every other method still works, and
/// the performance tool needs to be able to say why its ring is empty.
fn pump_frames(session: &Session, client: &DevtoolsClient) {
    let frames = match client.subscribe_frame_stats() {
        Ok(frames) => frames,
        Err(err) => {
            session.set_devtools_error(format!("frame-stats subscription failed: {err:#}"));
            return;
        }
    };

    while !session.stopping() {
        match frames.recv_timeout(FRAME_POLL_SLICE) {
            Ok(frame) => session.push_frame(frame),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use frust_drive::build_info::BuildMode;
    use frust_drive::devices::{Device, Kind, Platform};
    use frust_drive::process::{FakeProcessRunner, Output};

    use super::super::session::{RunTarget, SessionId};
    use super::super::test_support::RecordingRunner;
    use super::*;

    const SERIAL: &str = "emulator-5554";
    const DEVICE_PORT: u16 = 9_999;
    const HOST_PORT: u16 = 41_234;

    fn android_session() -> Arc<Session> {
        let device = Device {
            id: SERIAL.to_string(),
            name: "Android Emulator".to_string(),
            platform: Platform::Android,
            kind: Kind::Emulator,
            os_version: None,
            connection_state: None,
        };
        Arc::new(Session::new(
            SessionId(1),
            RunTarget::Android(device),
            BuildMode::Debug,
            PathBuf::from("/tmp/frust-mcp-devtools-test"),
        ))
    }

    fn adb_runner() -> Arc<RecordingRunner> {
        Arc::new(RecordingRunner::new(
            FakeProcessRunner::new()
                .with(
                    format!("adb -s {SERIAL} forward tcp:0 tcp:{DEVICE_PORT}"),
                    Output {
                        success: true,
                        stdout: format!("{HOST_PORT}\n"),
                        stderr: String::new(),
                    },
                )
                .with(
                    format!("adb -s {SERIAL} forward --remove"),
                    Output {
                        success: true,
                        stdout: String::new(),
                        stderr: String::new(),
                    },
                ),
        ))
    }

    /// A forward allocated after teardown has already swept the session is
    /// removed by the thread that allocated it — otherwise the host keeps an
    /// `adb forward` mapping nothing will ever take down.
    #[test]
    fn a_forward_allocated_after_teardown_is_removed_by_its_own_thread() {
        let session = android_session();
        let recorder = adb_runner();
        let runner: Runner = Arc::clone(&recorder) as Runner;

        // The stop lands while this thread is between "adb forward returned a
        // port" and "the session recorded it".
        session.request_stop();
        let _ = session.take_resources();

        let discovery = Discovery {
            port: DEVICE_PORT,
            token: None,
        };
        let connected = connect(&session, &runner, &discovery).expect("connect reports cleanly");

        assert!(
            connected.is_none(),
            "a stopped session must not connect its devtools client"
        );
        assert!(
            recorder
                .runs()
                .contains(&format!("adb -s {SERIAL} forward --remove tcp:{HOST_PORT}")),
            "the orphaned forward was never removed: {:?}",
            recorder.runs()
        );
    }
}
