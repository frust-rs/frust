//! Connected-DAP-client registry: an RAII-tracked count plus a connect-time
//! record for each live session.
//!
//! [`DapClientRegistry`] is what [`crate::serve_embedded`]'s caller (an
//! embedder like `frust-tui`) reads to answer "which editors are attached to
//! the debug adapter right now, and since when" — the same shape, and the same
//! guard discipline, as `frust_mcp::ClientRegistry`.
//!
//! # Why an entry starts unnamed
//!
//! A connection exists before it says who it is: the TCP accept loop builds
//! the adapter (and with it the registration) the moment a socket arrives,
//! while the client's name and id only turn up in the `initialize` arguments
//! one round trip later. So [`DapClientRegistry::register`] mints the entry
//! immediately — a connected client with no name is still a connected client,
//! and a port scanner that never speaks DAP is exactly what a host wants to
//! see — and [`DapClientGuard::identify`] fills the two fields in when the
//! handshake arrives. The guard's `Drop` is the only disconnect signal: it
//! fires when the per-connection adapter is dropped, which the session does on
//! every exit path.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use crate::protocol::types::InitializeRequestArguments;
use crate::sanitize::console_safe;

/// One connected DAP client's record.
#[derive(Debug, Clone)]
pub struct DapClientEntry {
    /// Opaque id minted by [`DapClientRegistry::register`] — the connection's
    /// identity in this registry, unrelated to the client's own `clientID`.
    pub id: u64,
    /// The client's human-readable name from `initialize` (`"Visual Studio
    /// Code"`), control-characters stripped. `None` until the handshake
    /// arrives — see the module doc.
    pub client_name: Option<String>,
    /// The client's own `clientID` from `initialize` (`"vscode"`),
    /// control-characters stripped. `None` until the handshake arrives.
    pub client_id: Option<String>,
    /// Wall-clock connect time, captured once via `SystemTime::now()` — for
    /// display only. Never compare two entries' `connected_at` to order them;
    /// use `connected_since`, since `SystemTime` is not guaranteed monotonic.
    pub connected_at: SystemTime,
    /// Monotonic connect time — the ordering-safe field, and what a caller
    /// should diff against `Instant::now()` for a "connected for" duration.
    pub connected_since: Instant,
}

/// Shared registry of currently-connected DAP clients.
///
/// Cheap to clone (an `Arc` pair inside) — one instance is meant to be created
/// once by an embedder and handed to every [`crate::serve_embedded`] call
/// across repeated start/stop cycles, or a fresh one built per call; either is
/// safe since nothing here is process-global state.
#[derive(Clone, Default)]
pub struct DapClientRegistry {
    entries: Arc<Mutex<Vec<DapClientEntry>>>,
    next_id: Arc<AtomicU64>,
}

impl DapClientRegistry {
    /// A registry with no connected clients.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mints a new client id, records its (still unnamed) [`DapClientEntry`],
    /// and returns the RAII [`DapClientGuard`] that removes it again on
    /// `Drop`.
    pub(crate) fn register(&self) -> DapClientGuard {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let entry = DapClientEntry {
            id,
            client_name: None,
            client_id: None,
            connected_at: SystemTime::now(),
            connected_since: Instant::now(),
        };
        if let Ok(mut entries) = self.entries.lock() {
            entries.push(entry);
        }
        DapClientGuard {
            id,
            registry: self.clone(),
        }
    }

    /// Every currently-connected client, oldest first.
    pub fn snapshot(&self) -> Vec<DapClientEntry> {
        self.entries.lock().map(|e| e.clone()).unwrap_or_default()
    }

    /// The number of currently-connected clients.
    pub fn count(&self) -> usize {
        self.entries.lock().map(|e| e.len()).unwrap_or(0)
    }

    /// Records who `id` turned out to be — called once, from
    /// [`DapClientGuard::identify`], when `initialize` arrives.
    fn identify(&self, id: u64, client: &InitializeRequestArguments) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) else {
            return;
        };
        // Client-chosen strings that a host will render: sanitized here, at
        // the one place they enter the registry, rather than at every reader.
        entry.client_name = client.client_name.as_deref().map(console_safe);
        entry.client_id = client.client_id.as_deref().map(console_safe);
    }

    /// Removes `id`'s entry — called only from [`DapClientGuard::drop`].
    fn remove(&self, id: u64) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|entry| entry.id != id);
        }
    }
}

/// RAII registration of one connected DAP client.
///
/// Moved into the per-connection [`crate::OrchestrationAdapter`] by
/// [`crate::serve_embedded`]; removes its [`DapClientEntry`] when that adapter
/// is dropped, which the session does after `on_disconnect` on every exit
/// path.
pub(crate) struct DapClientGuard {
    id: u64,
    registry: DapClientRegistry,
}

impl DapClientGuard {
    /// Fills in the client's name and id from its `initialize` arguments.
    pub(crate) fn identify(&self, client: &InitializeRequestArguments) {
        self.registry.identify(self.id, client);
    }
}

impl Drop for DapClientGuard {
    fn drop(&mut self) {
        self.registry.remove(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(name: &str, id: &str) -> InitializeRequestArguments {
        InitializeRequestArguments {
            client_id: Some(id.to_owned()),
            client_name: Some(name.to_owned()),
            ..InitializeRequestArguments::default()
        }
    }

    #[test]
    fn a_fresh_registry_has_no_clients() {
        let registry = DapClientRegistry::new();
        assert_eq!(registry.count(), 0);
        assert!(registry.snapshot().is_empty());
    }

    #[test]
    fn register_adds_an_entry_that_is_not_yet_named() {
        let registry = DapClientRegistry::new();
        let _guard = registry.register();

        let snapshot = registry.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert!(snapshot[0].client_name.is_none(), "named before initialize");
        assert!(snapshot[0].connected_at <= SystemTime::now());
        assert!(snapshot[0].connected_since.elapsed().as_secs() < 5);
    }

    #[test]
    fn identify_names_the_entry_and_strips_control_characters() {
        let registry = DapClientRegistry::new();
        let guard = registry.register();

        guard.identify(&client("Visual \u{1b}[31mStudio Code", "vscode"));

        let snapshot = registry.snapshot();
        assert_eq!(snapshot[0].client_id.as_deref(), Some("vscode"));
        let name = snapshot[0].client_name.clone().expect("a named client");
        assert!(
            !name.contains('\u{1b}'),
            "an escape reached the host: {name}"
        );
        assert!(name.contains("Studio Code"), "{name}");
    }

    #[test]
    fn dropping_the_guard_removes_the_entry() {
        let registry = DapClientRegistry::new();
        let guard = registry.register();
        assert_eq!(registry.count(), 1);

        drop(guard);

        assert_eq!(registry.count(), 0);
        assert!(registry.snapshot().is_empty());
    }

    #[test]
    fn each_registration_mints_a_distinct_id() {
        let registry = DapClientRegistry::new();
        let first = registry.register();
        let second = registry.register();

        assert_ne!(first.id, second.id);
        assert_eq!(registry.count(), 2);
    }

    #[test]
    fn dropping_one_of_several_guards_leaves_the_others() {
        let registry = DapClientRegistry::new();
        let first = registry.register();
        let second = registry.register();

        drop(first);

        assert_eq!(registry.count(), 1);
        assert_eq!(registry.snapshot()[0].id, second.id);
    }

    #[test]
    fn clones_of_the_registry_share_state() {
        let registry = DapClientRegistry::new();
        let clone = registry.clone();

        let guard = registry.register();
        assert_eq!(clone.count(), 1);

        drop(guard);
        assert_eq!(clone.count(), 0);
    }
}
