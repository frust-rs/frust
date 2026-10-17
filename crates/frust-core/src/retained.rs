//! Retained-page keys and the resident-key ledger.
//!
//! The renderer publishes a [`RetainedLedger`] of the pages it holds after each
//! frame; the shell hands it to `RenderRoot::set_retained_ledger` before the next
//! paint, so it is one frame stale by design. A page resident under an older
//! recording is invisible to the renderer (a newer scene can be dropped on the
//! way), so a widget that recorded at paint serial `n` draws the page only when
//! the ledger [covers](RetainedLedger::covers) `n`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// `Relaxed` suffices: the counter only produces distinct values.
static NEXT_RETAINED_KEY: AtomicU64 = AtomicU64::new(0);

/// One retained page's identity; minted from one process-wide counter, since
/// the renderer caches by key alone and a shared key would alias content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RetainedKey(pub u64);

impl RetainedKey {
    pub fn mint() -> Self {
        Self(NEXT_RETAINED_KEY.fetch_add(1, Ordering::Relaxed))
    }

    /// Reserve `count` consecutive keys and return the first.
    pub fn mint_range(count: u64) -> Self {
        Self(NEXT_RETAINED_KEY.fetch_add(count, Ordering::Relaxed))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for RetainedKey {
    fn from(raw: u64) -> Self {
        Self(raw)
    }
}

/// What the renderer holds for one resident page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetainedEntry {
    /// Rasterized extent, in physical pixels.
    pub extent_px: (u32, u32),
    pub scale: f32,
}

/// The retained pages the renderer reported resident after a frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RetainedLedger {
    /// Resident pages by raw [`RetainedKey`].
    pub entries: HashMap<u64, RetainedEntry>,
    /// Paint serial of the last scene whose retained-store commit completed
    /// (`0` before any); a refused or failed frame does not advance it.
    pub frame: u64,
}

impl RetainedLedger {
    const SCALE_EPSILON: f32 = 1e-3;

    /// Nothing resident: what every shell starts with.
    pub fn empty() -> Self {
        Self::default()
    }

    /// From `(key, extent_px, scale)` triples, after serial `frame` committed.
    pub fn from_entries(
        frame: u64,
        entries: impl IntoIterator<Item = (u64, (u32, u32), f32)>,
    ) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(key, extent_px, scale)| (key, RetainedEntry { extent_px, scale }))
                .collect(),
            frame,
        }
    }

    /// `frame >= serial`; serial `0` (an unnumbered paint) is always covered.
    pub fn covers(&self, serial: u64) -> bool {
        self.frame >= serial
    }

    /// Whether `key` is resident at exactly `extent_px` and `scale` (within `1e-3`).
    pub fn is_resident(&self, key: RetainedKey, extent_px: (u32, u32), scale: f32) -> bool {
        self.entries.get(&key.0).is_some_and(|e| {
            e.extent_px == extent_px && (e.scale - scale).abs() <= Self::SCALE_EPSILON
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_keys_are_distinct_and_ranges_are_disjoint() {
        let a = RetainedKey::mint();
        let b = RetainedKey::mint();
        assert_ne!(a, b);
        let r = RetainedKey::mint_range(2);
        let c = RetainedKey::mint();
        assert!(c.get() >= r.get() + 2);
    }

    #[test]
    fn lookups_require_exact_extent_and_close_scale() {
        let key = RetainedKey(7);
        let ledger = RetainedLedger::from_entries(3, [(7, (100, 50), 2.0)]);
        assert!(ledger.is_resident(key, (100, 50), 2.0));
        assert!(ledger.is_resident(key, (100, 50), 2.0005));
        assert!(!ledger.is_resident(key, (101, 50), 2.0));
        assert!(!ledger.is_resident(key, (100, 50), 2.01));
        assert!(!ledger.is_resident(RetainedKey(8), (100, 50), 2.0));
    }

    #[test]
    fn a_ledger_covers_its_own_serial_and_every_earlier_one() {
        let ledger = RetainedLedger::from_entries(5, [(7, (1, 1), 1.0)]);
        assert!(ledger.covers(0));
        assert!(ledger.covers(4));
        assert!(ledger.covers(5));
        assert!(!ledger.covers(6), "a later recording is not vouched for");
        assert!(RetainedLedger::empty().covers(0));
        assert!(!RetainedLedger::empty().covers(1));
    }

    #[test]
    fn empty_ledger_is_never_resident() {
        let ledger = RetainedLedger::empty();
        assert!(!ledger.is_resident(RetainedKey(0), (0, 0), 1.0));
        assert!(!ledger.is_resident(RetainedKey(1), (10, 10), 1.0));
    }
}
