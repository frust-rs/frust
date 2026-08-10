//! Connected-MCP-client registry: an RAII-tracked count plus a
//! connect-time record for each live session.
//!
//! [`ClientRegistry`] is what [`crate::server::serve_embedded`]'s caller
//! (an embedder like `frust-tui`) reads to answer "how many MCP clients are
//! connected right now, and since when" — the same shape fdemon-pro's
//! `ClientTracker` feeds its TUI status badge from, minus the badge-message
//! dispatch (this crate has no host UI to notify).
//!
//! # Why the id is minted here, not read off the rmcp session
//!
//! The streamable-HTTP transport's service factory is a zero-argument
//! `Fn() -> Result<S, io::Error>` — it runs once per new MCP session but is
//! never handed that session's id, so there is nothing to key an entry on
//! except an id this registry mints itself. [`ClientRegistry::register`]
//! does exactly that and returns a [`ClientGuard`] the caller moves into
//! the per-session handler; the guard's `Drop` is consequently the *only*
//! disconnect signal this crate has. A client that vanishes without rmcp
//! ever tearing its session down (a killed process, a dropped connection
//! rmcp doesn't notice) leaves its entry in the registry until the server
//! itself stops — a documented limitation, not a bug to chase here.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

/// One connected MCP client's record.
#[derive(Debug, Clone)]
pub struct ClientEntry {
    /// Opaque id minted by [`ClientRegistry::register`] — see the module
    /// doc for why this isn't the rmcp session id.
    pub id: u64,
    /// Wall-clock connect time, captured once via `SystemTime::now()` — for
    /// display only. Never compare two entries' `connected_at` to order
    /// them; use `connected_since` instead, since `SystemTime` is not
    /// guaranteed monotonic (an NTP step or clock change can move it
    /// backwards mid-run).
    pub connected_at: SystemTime,
    /// Monotonic connect time — the ordering-safe field, and what a caller
    /// should diff against `Instant::now()` for a "connected for" duration.
    pub connected_since: Instant,
}

/// Shared registry of currently-connected MCP clients.
///
/// Cheap to clone (an `Arc` pair inside) — one instance is meant to be
/// created once by an embedder and handed to every [`crate::serve_embedded`]
/// call across repeated start/stop cycles, or a fresh one built per call;
/// either is safe since nothing here is process-global state.
#[derive(Clone, Default)]
pub struct ClientRegistry {
    entries: Arc<Mutex<Vec<ClientEntry>>>,
    next_id: Arc<AtomicU64>,
}

impl ClientRegistry {
    /// A registry with no connected clients.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mints a new client id, records its [`ClientEntry`], and returns the
    /// RAII [`ClientGuard`] that removes it again on `Drop` — the service
    /// factory's disconnect signal (see the module doc).
    pub(crate) fn register(&self) -> ClientGuard {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let entry = ClientEntry {
            id,
            connected_at: SystemTime::now(),
            connected_since: Instant::now(),
        };
        if let Ok(mut entries) = self.entries.lock() {
            entries.push(entry);
        }
        ClientGuard {
            id,
            registry: self.clone(),
        }
    }

    /// Every currently-connected client, oldest first.
    pub fn snapshot(&self) -> Vec<ClientEntry> {
        self.entries.lock().map(|e| e.clone()).unwrap_or_default()
    }

    /// The number of currently-connected clients.
    pub fn count(&self) -> usize {
        self.entries.lock().map(|e| e.len()).unwrap_or(0)
    }

    /// Removes `id`'s entry — called only from [`ClientGuard::drop`].
    fn remove(&self, id: u64) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.retain(|entry| entry.id != id);
        }
    }
}

/// RAII registration of one connected MCP client.
///
/// Moved into the per-session handler by the service factory
/// (`server.rs`'s `serve_embedded`); removes its [`ClientEntry`] from the
/// [`ClientRegistry`] when that handler's last `Arc` clone is dropped.
pub(crate) struct ClientGuard {
    id: u64,
    registry: ClientRegistry,
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        self.registry.remove(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_registry_has_no_clients() {
        let registry = ClientRegistry::new();
        assert_eq!(registry.count(), 0);
        assert!(registry.snapshot().is_empty());
    }

    #[test]
    fn register_adds_a_populated_entry() {
        let registry = ClientRegistry::new();
        let guard = registry.register();

        assert_eq!(registry.count(), 1);
        let snapshot = registry.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].id, guard_id(&guard));
        assert!(snapshot[0].connected_at <= SystemTime::now());
        assert!(snapshot[0].connected_since.elapsed().as_secs() < 5);
    }

    #[test]
    fn dropping_the_guard_removes_the_entry() {
        let registry = ClientRegistry::new();
        let guard = registry.register();
        assert_eq!(registry.count(), 1);

        drop(guard);

        assert_eq!(registry.count(), 0);
        assert!(registry.snapshot().is_empty());
    }

    #[test]
    fn each_registration_mints_a_distinct_id() {
        let registry = ClientRegistry::new();
        let first = registry.register();
        let second = registry.register();

        assert_ne!(guard_id(&first), guard_id(&second));
        assert_eq!(registry.count(), 2);
    }

    #[test]
    fn dropping_one_of_several_guards_leaves_the_others() {
        let registry = ClientRegistry::new();
        let first = registry.register();
        let second = registry.register();

        drop(first);

        assert_eq!(registry.count(), 1);
        let snapshot = registry.snapshot();
        assert_eq!(snapshot[0].id, guard_id(&second));
    }

    #[test]
    fn clones_of_the_registry_share_state() {
        let registry = ClientRegistry::new();
        let clone = registry.clone();

        let guard = registry.register();
        assert_eq!(clone.count(), 1);

        drop(guard);
        assert_eq!(clone.count(), 0);
    }

    /// Test-only accessor: [`ClientGuard::id`] is otherwise private, since
    /// production code never needs to read it back off the guard.
    fn guard_id(guard: &ClientGuard) -> u64 {
        guard.id
    }
}
