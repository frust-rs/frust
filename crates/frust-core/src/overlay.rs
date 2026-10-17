//! The overlay portal: how a widget anywhere in the tree floats a pod above the
//! whole app (menus, popovers, tooltips, selection toolbars).
//!
//! The owner builds, lays out and keeps the pod and registers an [`OverlayEntry`]
//! from its `paint`; the root paints every entry after the main tree, unclipped.
//!
//! Per-pass registry contract: the root clears the registry every paint, so an
//! owner re-registers each frame to stay up. The root hit-tests registered rects
//! first; a hit is an [`InputEvent::Overlay`](crate::event::InputEvent::Overlay)
//! broadcast that only the owner holding the key acts on.
//!
//! A key the focus path declines goes to the focus session's forward pod, then (an Escape) to
//! the topmost [`OverlayKeys::Modal`] pod, both as `OverlayEventKind::Key`, then to the shortcut
//! table, masked to that modal pod's own declarations while one is registered.
//!
//! # Not in v1
//!
//! * No focus trap: a modal pod masks shortcuts only; focus is neither confined nor restored.
//! * Not seen by [`inspect`](crate::app::RenderRoot::inspect) unless the owner visits the pod.
//! * No nesting: a registration made from inside a floated pod's paint is dropped;
//!   register a submenu from the same owner, in the main tree's paint.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::Rc;

use kurbo::Rect;

use crate::event::PassBracket;
use crate::insets::WindowInsets;
use crate::widget::ChildPod;

thread_local! {
    /// The next [`OverlayKey::next`] value; starts at `1` so `0` can mean "none".
    static NEXT_OVERLAY_KEY: Cell<u64> = const { Cell::new(1) };

    /// Entries registered so far in this thread's open paint pass, in order.
    static OVERLAY_REGISTRY: Cell<Vec<OverlayEntry>> = const { Cell::new(Vec::new()) };

    static OVERLAY_PASS_OPEN: Cell<bool> = const { Cell::new(false) };
}

/// An overlay owner's identity, quoted back by every overlay event routed to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OverlayKey(u64);

impl OverlayKey {
    /// Allocate a key unique on this thread. Call once when the owner is built.
    pub fn next() -> Self {
        OverlayKey(NEXT_OVERLAY_KEY.with(|next| {
            let id = next.get();
            next.set(id.wrapping_add(1));
            id
        }))
    }

    /// The raw integer, for map keys; allocation order is not a contract.
    pub fn value(self) -> u64 {
        self.0
    }
}

/// A surface's z-band: `Floating` paints below `Tooltip`, which hit-tests first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OverlayBand {
    /// Menus, popovers, dropdowns, selection toolbars.
    Floating,
    /// Tooltips and other chrome above everything else.
    Tooltip,
}

/// Whether a registered surface takes pointer input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OverlayInput {
    /// Hit-tested: a pointer inside the rect goes to the owner, not the main tree.
    Interactive,
    /// Painted but never hit-tested, and never sent any overlay event.
    Transparent,
}

/// What a surface hears about a press that landed on nothing floated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OutsideTap {
    /// Nothing.
    Ignore,
    /// Send `OverlayEventKind::OutsideDown` on a primary press outside every rect.
    Notify {
        /// Whether the press stops there instead of also reaching the main tree.
        consume: bool,
    },
}

/// Whether a registered surface claims the keys the focus path declines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OverlayKeys {
    #[default]
    Pass,
    /// Masks main-tree shortcuts and takes a declined Escape (implied by a consuming `Notify`).
    Modal,
}

/// The shared handle the owner and the root reach one floated pod through.
pub type OverlayPod = Rc<RefCell<ChildPod>>;

/// One floated surface, registered by its owner for the paint pass in progress.
#[derive(Clone)]
pub struct OverlayEntry {
    pub key: OverlayKey,
    pub band: OverlayBand,
    pub input: OverlayInput,
    pub outside_tap: OutsideTap,
    pub keys: OverlayKeys,
    /// Absolute logical window rect (from `PaintCtx::origin`): where the pod
    /// paints and what routing hit-tests.
    pub window_rect: Rect,
    /// The pod the root paints; the owner lays it out but must not paint it.
    pub pod: OverlayPod,
    /// The insets the pod was laid out under; paint installs the same value.
    pub insets: WindowInsets,
}

impl fmt::Debug for OverlayEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverlayEntry")
            .field("key", &self.key)
            .field("band", &self.band)
            .field("input", &self.input)
            .field("outside_tap", &self.outside_tap)
            .field("keys", &self.keys)
            .field("window_rect", &self.window_rect)
            .field("insets", &self.insets)
            .field("pod", &"<pod>")
            .finish()
    }
}

/// An [`OverlayEntry`]'s routing half, kept between passes; no pod, so no co-ownership.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayHit {
    pub key: OverlayKey,
    pub band: OverlayBand,
    pub input: OverlayInput,
    pub outside_tap: OutsideTap,
    pub keys: OverlayKeys,
    pub window_rect: Rect,
}

impl OverlayHit {
    pub(crate) fn of(entry: &OverlayEntry) -> Self {
        Self {
            key: entry.key,
            band: entry.band,
            input: entry.input,
            outside_tap: entry.outside_tap,
            keys: entry.keys,
            window_rect: entry.window_rect,
        }
    }

    /// Interactive and [`OverlayKeys::Modal`], or consuming outside taps.
    pub(crate) fn is_modal(&self) -> bool {
        self.input == OverlayInput::Interactive
            && (self.keys == OverlayKeys::Modal
                || self.outside_tap == OutsideTap::Notify { consume: true })
    }

    /// Half-open on the far edges, so surfaces sharing an edge never both hit.
    pub(crate) fn contains(&self, point: kurbo::Point) -> bool {
        point.x >= self.window_rect.x0
            && point.x < self.window_rect.x1
            && point.y >= self.window_rect.y0
            && point.y < self.window_rect.y1
    }
}

/// The [`PassBracket`] around one registration pass: enter clears, `take` drains.
pub(crate) struct OverlayPaintPass(PassBracket<Vec<OverlayEntry>>);

impl OverlayPaintPass {
    pub(crate) fn enter() -> Self {
        OverlayPaintPass(PassBracket::enter(&OVERLAY_REGISTRY, &OVERLAY_PASS_OPEN))
    }

    pub(crate) fn take(&self) -> Vec<OverlayEntry> {
        self.0.take()
    }
}

/// Record `entry` in this thread's open pass (behind `PaintCtx::register_overlay`).
pub(crate) fn register(entry: OverlayEntry) {
    OVERLAY_REGISTRY.with(|registry| {
        let mut entries = registry.take();
        entries.push(entry);
        registry.set(entries);
    });
}

/// Stable sort by band: later registrations paint above earlier; reverse to hit-test.
pub(crate) fn sort_into_paint_order(entries: &mut [OverlayEntry]) {
    entries.sort_by_key(|entry| entry.band);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::BoxConstraints;
    use crate::widget::{LayoutCtx, PaintCtx, PaintScene, Widget};
    use kurbo::{Point, Size};

    /// A do-nothing leaf, so an entry can carry a real [`ChildPod`].
    struct StubWidget;
    impl Widget for StubWidget {
        fn layout(&mut self, _ctx: &mut LayoutCtx, _bc: &BoxConstraints) -> Size {
            Size::ZERO
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    fn entry(key: OverlayKey, band: OverlayBand, rect: Rect) -> OverlayEntry {
        OverlayEntry {
            key,
            band,
            input: OverlayInput::Interactive,
            outside_tap: OutsideTap::Ignore,
            keys: OverlayKeys::Pass,
            window_rect: rect,
            pod: Rc::new(RefCell::new(ChildPod::new(Box::new(StubWidget)))),
            insets: WindowInsets::default(),
        }
    }

    #[test]
    fn keys_are_distinct_and_stable() {
        let first = OverlayKey::next();
        let second = OverlayKey::next();
        assert_ne!(first, second, "two owners never share an identity");
        // An owner stores its key; copying it around must not re-allocate.
        let stored = first;
        assert_eq!(stored, first);
        assert_ne!(stored, second);
    }

    #[test]
    fn paint_order_puts_floating_below_tooltip_and_is_stable_within_a_band() {
        let (a, b, c, d) = (
            OverlayKey::next(),
            OverlayKey::next(),
            OverlayKey::next(),
            OverlayKey::next(),
        );
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        // Registered tooltip-first and interleaved, which is exactly what the
        // sort has to survive: the band decides, registration order only breaks
        // ties inside one.
        let mut entries = vec![
            entry(a, OverlayBand::Tooltip, rect),
            entry(b, OverlayBand::Floating, rect),
            entry(c, OverlayBand::Tooltip, rect),
            entry(d, OverlayBand::Floating, rect),
        ];
        sort_into_paint_order(&mut entries);
        assert_eq!(
            entries.iter().map(|e| e.key).collect::<Vec<_>>(),
            vec![b, d, a, c],
            "Floating first (in registration order), then Tooltip (in registration order)"
        );
    }

    #[test]
    fn a_hit_rect_is_half_open_on_its_far_edges() {
        let hit = OverlayHit::of(&entry(
            OverlayKey::next(),
            OverlayBand::Floating,
            Rect::new(10.0, 20.0, 110.0, 60.0),
        ));
        assert!(
            hit.contains(Point::new(10.0, 20.0)),
            "near edges are inside"
        );
        assert!(hit.contains(Point::new(109.9, 59.9)));
        // Half-open like `ChildPod::contains`, so two surfaces sharing an edge
        // cannot both claim the same pixel.
        assert!(!hit.contains(Point::new(110.0, 40.0)));
        assert!(!hit.contains(Point::new(60.0, 60.0)));
        assert!(!hit.contains(Point::new(9.9, 40.0)));
    }

    #[test]
    fn a_hit_keeps_the_routing_fields_and_drops_the_pod() {
        let key = OverlayKey::next();
        let mut source = entry(key, OverlayBand::Tooltip, Rect::new(1.0, 2.0, 3.0, 4.0));
        source.input = OverlayInput::Transparent;
        source.outside_tap = OutsideTap::Notify { consume: true };
        let hit = OverlayHit::of(&source);
        assert_eq!(hit.key, key);
        assert_eq!(hit.band, OverlayBand::Tooltip);
        assert_eq!(hit.input, OverlayInput::Transparent);
        assert_eq!(hit.outside_tap, OutsideTap::Notify { consume: true });
        assert_eq!(hit.window_rect, source.window_rect);
        // The pod stayed with its owner: the entry still holds the only two
        // clones (the owner's and this local one), never a third in the root.
        assert_eq!(Rc::strong_count(&source.pod), 1);
    }

    #[test]
    fn a_hit_is_modal_when_interactive_and_modal_or_consuming_outside_taps() {
        let mut source = entry(OverlayKey::next(), OverlayBand::Floating, Rect::ZERO);
        assert!(!OverlayHit::of(&source).is_modal());
        source.keys = OverlayKeys::Modal;
        assert!(OverlayHit::of(&source).is_modal());
        source.keys = OverlayKeys::Pass;
        source.outside_tap = OutsideTap::Notify { consume: true };
        assert!(OverlayHit::of(&source).is_modal());
        source.input = OverlayInput::Transparent;
        assert!(
            !OverlayHit::of(&source).is_modal(),
            "a transparent pod never takes keys"
        );
    }

    #[test]
    fn the_registry_is_pass_scoped_and_preserves_registration_order() {
        let (first, second) = (OverlayKey::next(), OverlayKey::next());
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let pass = OverlayPaintPass::enter();
        assert!(pass.take().is_empty(), "a fresh pass starts empty");

        register(entry(first, OverlayBand::Floating, rect));
        register(entry(second, OverlayBand::Floating, rect));
        let drained = pass.take();
        assert_eq!(
            drained.iter().map(|e| e.key).collect::<Vec<_>>(),
            vec![first, second],
            "registration order is preserved"
        );
        assert!(pass.take().is_empty(), "and the drain empties the registry");
        drop(pass);

        // A registration made with no pass open (a leaf unit test painting a bare
        // `PaintCtx`) is dropped by the next pass's clear, never leaked into it —
        // which is what makes "the registry is empty at the start of every paint"
        // true without anyone unregistering.
        register(entry(first, OverlayBand::Floating, rect));
        let next = OverlayPaintPass::enter();
        assert!(
            next.take().is_empty(),
            "a stray registration does not survive into the next pass"
        );
    }

    #[test]
    fn a_nested_pass_hands_the_enclosing_passs_registrations_back() {
        let (outer_key, inner_key) = (OverlayKey::next(), OverlayKey::next());
        let rect = Rect::new(0.0, 0.0, 10.0, 10.0);
        let outer = OverlayPaintPass::enter();
        register(entry(outer_key, OverlayBand::Floating, rect));
        {
            let inner = OverlayPaintPass::enter();
            assert!(inner.take().is_empty(), "the inner pass starts empty");
            register(entry(inner_key, OverlayBand::Floating, rect));
            assert_eq!(
                inner.take().iter().map(|e| e.key).collect::<Vec<_>>(),
                vec![inner_key]
            );
        }
        assert_eq!(
            outer.take().iter().map(|e| e.key).collect::<Vec<_>>(),
            vec![outer_key],
            "the enclosing pass's registrations are handed back intact"
        );
    }

    #[test]
    fn debug_prints_the_routing_fields_without_the_pod() {
        let rendered = format!(
            "{:?}",
            entry(
                OverlayKey::next(),
                OverlayBand::Floating,
                Rect::new(0.0, 0.0, 10.0, 10.0)
            )
        );
        assert!(rendered.contains("Floating"), "{rendered}");
        assert!(rendered.contains("Interactive"), "{rendered}");
        assert!(rendered.contains("<pod>"), "{rendered}");
    }
}
