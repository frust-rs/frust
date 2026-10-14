//! Retained-page keys and the resident-key ledger.
//!
//! A retained page is a body the renderer rasterized once and keeps in a cache
//! keyed by a [`RetainedKey`]. Whether a page is still resident is the
//! renderer's knowledge, so it publishes a [`RetainedLedger`] after each frame
//! and the shell hands the latest one to [`RenderRoot::set_retained_ledger`]
//! before the next paint; a widget then asks
//! [`PaintCtx::retained_available`](crate::PaintCtx::retained_available) whether
//! to draw the page or paint the body again. The ledger is one frame stale by
//! design: the UI thread never waits on the render thread for it, and a stale
//! "resident" answer is repaired by the renderer's own refusal path.
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
    /// The renderer frame this ledger describes (diagnostic).
    pub frame: u64,
}

impl RetainedLedger {
    /// Tolerance when comparing the recorded and requested scale.
    const SCALE_EPSILON: f32 = 1e-3;

    /// An empty ledger: nothing is resident. What every shell starts with.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build a ledger from `(key, extent_px, scale)` triples.
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
    fn empty_ledger_is_never_resident() {
        let ledger = RetainedLedger::empty();
        assert!(!ledger.is_resident(RetainedKey(0), (0, 0), 1.0));
        assert!(!ledger.is_resident(RetainedKey(1), (10, 10), 1.0));
    }
}

/// Drives `RenderRoot::paint` with a ledger: a boundary-like widget nested one
/// pod deep (so the ledger must be threaded to children) reads
/// `PaintCtx::retained_available` and records the answers it saw.
#[cfg(test)]
mod paint_tests {
    use std::cell::RefCell;

    use crate::{
        AnyView, BoxConstraints, BuildCtx, ChangeFlags, ChildPod, DiscardScene, FrameTime,
        LayoutCtx, PaintCtx, PaintScene, RenderRoot, RetainedKey, RetainedLedger, View, Widget,
        any,
    };
    use kurbo::{Point, Size};
    use reactive_graph::owner::Owner;

    thread_local! {
        static ANSWERS: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
    }

    const KEY: RetainedKey = RetainedKey(42);
    /// The page the boundary asks about: 100x50 logical at scale 2 -> 200x100 px.
    const PAGE: Size = Size::new(200.0, 100.0);

    struct Boundary;

    impl Widget for Boundary {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.constrain(Size::new(100.0, 50.0))
        }
        fn paint(&mut self, ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {
            let available = ctx.retained_available(KEY, PAGE, 2.0);
            ANSWERS.with(|a| a.borrow_mut().push(available));
        }
    }

    struct BoundaryView;

    impl<S: 'static> View<S> for BoundaryView {
        type Element = Boundary;
        fn build(&self, _ctx: &mut BuildCtx<'_>) -> Boundary {
            Boundary
        }
        fn rebuild(&self, _: &Self, _: &mut Boundary, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    struct Host {
        child: ChildPod,
    }

    impl Widget for Host {
        fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            let size = self
                .child
                .layout_child(ctx, &BoxConstraints::loose(bc.max()));
            self.child.set_origin(Point::ZERO);
            bc.constrain(size)
        }
        fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
            self.child.paint_child(ctx, scene);
        }
        fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
            visitor(&self.child);
        }
    }

    struct HostView;

    impl<S: 'static> View<S> for HostView {
        type Element = Host;
        fn build(&self, ctx: &mut BuildCtx<'_>) -> Host {
            let child: AnyView<S> = any(BoundaryView);
            Host {
                child: ChildPod::new(Box::new(child.build(ctx))),
            }
        }
        fn rebuild(&self, _: &Self, _: &mut Host, _ctx: &mut BuildCtx<'_>) -> ChangeFlags {
            ChangeFlags::NONE
        }
    }

    fn paint_once(root: &mut RenderRoot<(), HostView>) -> bool {
        ANSWERS.with(|a| a.borrow_mut().clear());
        root.layout(Size::new(300.0, 300.0));
        root.paint(&mut DiscardScene, FrameTime::ZERO);
        ANSWERS.with(|a| *a.borrow().last().expect("the boundary painted"))
    }

    fn root() -> (RenderRoot<(), HostView>, Owner) {
        let owner = Owner::new();
        owner.set();
        let mut root = RenderRoot::new();
        root.rebuild(&mut |_: &mut ()| HostView, &mut ());
        (root, owner)
    }

    fn resident() -> RetainedLedger {
        RetainedLedger::from_entries(1, [(KEY.get(), (200, 100), 2.0)])
    }

    #[test]
    fn default_empty_ledger_reports_nothing_available() {
        let (mut root, _owner) = root();
        assert!(!paint_once(&mut root));
    }

    #[test]
    fn a_boundary_nested_under_a_host_sees_the_published_ledger() {
        let (mut root, _owner) = root();
        root.set_retained_ledger(&resident());
        assert!(paint_once(&mut root));
        // Replaced by a ledger without the key: no longer available.
        root.set_retained_ledger(&RetainedLedger::empty());
        assert!(!paint_once(&mut root));
    }

    #[test]
    fn extent_or_scale_mismatch_is_not_available() {
        let (mut root, _owner) = root();
        root.set_retained_ledger(&RetainedLedger::from_entries(
            1,
            [(KEY.get(), (199, 100), 2.0)],
        ));
        assert!(!paint_once(&mut root));
        root.set_retained_ledger(&RetainedLedger::from_entries(
            1,
            [(KEY.get(), (200, 100), 3.0)],
        ));
        assert!(!paint_once(&mut root));
    }

    #[test]
    fn the_kill_switch_makes_retained_available_false() {
        let (mut root, _owner) = root();
        root.set_retained_ledger(&resident());
        root.set_retained_layers(false);
        assert!(!paint_once(&mut root));
        root.set_retained_layers(true);
        assert!(paint_once(&mut root));
    }
}
