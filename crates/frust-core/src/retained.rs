//! Retained-page keys and the resident-key ledger.
//!
//! A retained page is a body the renderer rasterized once and keeps in a cache
//! keyed by a [`RetainedKey`]. Whether a page is still resident is the
//! renderer's knowledge, so it publishes a [`RetainedLedger`] after each frame
//! and the shell hands the latest one to [`RenderRoot::set_retained_ledger`]
//! before the next paint; a widget then asks
//! [`PaintCtx::retained_available`](crate::PaintCtx::retained_available) whether
//! to draw the page or paint the body again. The ledger is one frame stale by
//! design: the UI thread never waits on the render thread for it.
//!
//! # Which recording a resident page holds
//!
//! Staleness has two shapes, and the renderer can repair only one of them. A
//! page the ledger lists that the renderer has since dropped is refused at
//! draw time (the frame presents nothing and the UI paints again against the
//! emptied ledger). A page that is resident but holds an **older recording**
//! than the body the widget last recorded is not detectable by the renderer at
//! all: the scene that carried the newer recording can be dropped on its way
//! to the renderer (the split's scene slot is latest-wins), leaving the old
//! page standing under the same key, extent and scale.
//!
//! The paint serial closes that second gap. Every
//! [`RenderRoot::paint`](crate::RenderRoot::paint) mints a serial (`1`, `2`,
//! ... — [`PaintCtx::paint_serial`](crate::PaintCtx::paint_serial),
//! [`PaintOutcome::paint_serial`](crate::PaintOutcome::paint_serial)); the
//! shell carries it with the scene and hands it to the renderer before the
//! encode, and the ledger's [`frame`](RetainedLedger::frame) is the serial of
//! the last scene whose retained-store commit the renderer completed. A widget
//! that recorded a page at serial `n` therefore draws it only when the ledger
//! [covers](RetainedLedger::covers) `n` — a ledger published before that
//! scene was committed (or after it was dropped and a later one committed
//! instead) cannot vouch for it.
//!
//! [`RenderRoot::set_retained_ledger`]: crate::RenderRoot::set_retained_ledger

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Process-wide source of [`RetainedKey`]s. `Relaxed` is sufficient: the
/// counter only produces distinct values and synchronizes nothing else.
static NEXT_RETAINED_KEY: AtomicU64 = AtomicU64::new(0);

/// The identity of one retained page. A renderer caches a page's rasterized
/// body by key alone, so two unrelated owners sharing a key would alias each
/// other's content; every key is minted from one process-wide counter so none
/// can collide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RetainedKey(pub u64);

impl RetainedKey {
    /// Mint a fresh key, distinct from every other key minted in the process.
    pub fn mint() -> Self {
        Self(NEXT_RETAINED_KEY.fetch_add(1, Ordering::Relaxed))
    }

    /// Reserve `count` consecutive keys and return the first (`first`,
    /// `first + 1`, ...). For an owner that needs a fixed set of related keys
    /// (a switcher's outgoing/incoming pair). `count == 0` reserves nothing.
    pub fn mint_range(count: u64) -> Self {
        Self(NEXT_RETAINED_KEY.fetch_add(count, Ordering::Relaxed))
    }

    /// The raw key as handed to `PaintScene::push_retained`/`draw_retained`.
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
    /// The page's rasterized extent in physical pixels.
    pub extent_px: (u32, u32),
    /// The device scale the page was rasterized at.
    pub scale: f32,
}

/// The set of retained pages the renderer reported resident after a frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RetainedLedger {
    /// Resident pages by raw [`RetainedKey`].
    pub entries: HashMap<u64, RetainedEntry>,
    /// The paint serial ([`PaintOutcome::paint_serial`](crate::PaintOutcome::paint_serial))
    /// of the most recent scene whose retained-store commit the renderer
    /// completed before publishing this ledger. A refused or failed frame does
    /// not advance it; the empty ledger a store clear, surface destroy or
    /// scale change publishes carries the last committed serial (`0` before
    /// any). See [`RetainedLedger::covers`].
    pub frame: u64,
}

impl RetainedLedger {
    /// Tolerance when comparing the recorded and requested scale.
    const SCALE_EPSILON: f32 = 1e-3;

    /// An empty ledger: nothing is resident. What every shell starts with.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build a ledger from `(key, extent_px, scale)` triples, describing the
    /// store after the scene painted at serial `frame` was committed.
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

    /// Whether the renderer had committed the scene painted at `serial` (or a
    /// later one) when it published this ledger: `frame >= serial`. Serial `0`
    /// — a paint no render root numbered — is covered by every ledger.
    pub fn covers(&self, serial: u64) -> bool {
        self.frame >= serial
    }

    /// Whether `key` is resident at exactly `extent_px` and (within `1e-3`)
    /// `scale`. A page rasterized at another size or scale is not reusable.
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
