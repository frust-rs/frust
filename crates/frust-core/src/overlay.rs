//! The overlay portal: the mechanism a widget anywhere in the tree floats a pod
//! above the whole app with.
//!
//! # The contract: owner-hosted, root-painted, root-routed
//!
//! A floating surface (a popover, a menu, a selection toolbar, a tooltip) has to
//! escape its owner's bounds in three different ways at once, and this module
//! splits the three between the owner and the root:
//!
//! * **Owner-hosted.** The pod is a plain [`ChildPod`] the *owner* builds, lays
//!   out (against [`LayoutCtx::window_size`](crate::widget::LayoutCtx::window_size),
//!   loosely) and keeps — so it retains its own widget state and sits inside the
//!   owner's reactive context, exactly like any other child. Nothing is
//!   re-parented and no second tree exists: the pod stays a logical child of its
//!   call site.
//! * **Root-painted.** The owner does **not** paint the pod. It hands the root a
//!   registration ([`OverlayEntry`]) from its own `paint`, through
//!   [`PaintCtx::register_overlay`](crate::widget::PaintCtx::register_overlay),
//!   and [`RenderRoot::paint`](crate::app::RenderRoot::paint) paints every
//!   registered pod *after* the main tree — which is the only way a pod escapes
//!   both its owner's paint order and every ancestor's clip. Because the entry
//!   carries an absolute [`OverlayEntry::window_rect`] the owner computed from
//!   its own [`PaintCtx::origin`](crate::widget::PaintCtx::origin), an anchor
//!   follows the owner for free: the owner re-registers every frame from
//!   wherever it now paints.
//! * **Root-routed.** Painting last is not enough, because hit testing is
//!   bounds-gated: a pointer over the floated pod lands on whatever the *main*
//!   tree has at that position. So the root hit-tests the registered rects
//!   **first** (topmost band first) and, on a hit, dispatches
//!   [`InputEvent::Overlay`](crate::event::InputEvent::Overlay) as a broadcast
//!   instead of the original event. The broadcast reaches the owner wherever it
//!   sits, and the owner forwards the window-space payload into its pod. Nothing
//!   is hit-tested against the owner's own bounds, and — because a broadcast is
//!   not user input at the root — the main tree's focus session is untouched:
//!   tapping a popover does not blur the field that opened it.
//!
//! # Per-pass entries
//!
//! The registry is **per paint pass**: it is cleared when
//! [`RenderRoot::paint`](crate::app::RenderRoot::paint) begins and drained when
//! the main tree has finished painting. An owner keeps a surface alive by
//! registering it again every frame; an owner that stops registering disappears
//! from the root's routing table after the next paint, with nothing to
//! unregister and no way to leak an entry whose owner has been unmounted. A
//! kept-mounted exit animation is therefore just "keep registering while the
//! animation runs".
//!
//! The routing table the root retains between passes
//! ([`OverlayHit`]) deliberately carries **no pod handle** — only the key, the
//! band, the input class, the outside-tap policy and the rect. The owner holds
//! the pod; the root never does, so a pod can never outlive its owner because
//! the root kept a clone of it, and no [`RefCell`] borrow is ever held across a
//! pass boundary.
//!
//! # Not in v1
//!
//! * **No focus trap.** Focus inside a pod behaves exactly like focus anywhere
//!   else; nothing confines traversal to the pod or restores it on dismiss.
//! * **No declined-key forwarding.** Key/IME/edit-command events stay
//!   focus-routed and are never re-offered to an overlay owner that did not take
//!   focus.
//! * **Not visible to [`inspect`](crate::app::RenderRoot::inspect).** An overlay
//!   pod is not reachable through [`Widget::visit_children`](crate::widget::Widget::visit_children)
//!   unless its owner chooses to visit it, so devtools sees the owner, not the
//!   floated surface.
//! * **No nesting.** The registry is drained once, after the main tree paints, so
//!   a registration made from *inside* a floated pod's own paint is not painted
//!   this pass — and, because the pass's bracket clears the slot as it closes, it
//!   is dropped rather than leaked into the next one. A surface that itself needs
//!   a floated surface (a menu opening a submenu) registers both from the one
//!   owner, in the main tree's paint.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::Rc;

use kurbo::Rect;

use crate::event::PassBracket;
use crate::widget::ChildPod;

thread_local! {
    /// The next value [`OverlayKey::next`] hands out.
    ///
    /// Thread-local and UI-thread-affine, mirroring
    /// [`ChildPod`]'s own `NEXT_INSPECT_ID` allocator: the widget tree is
    /// single-threaded, and an owner allocates its key while building itself
    /// with no root in scope to ask. Two threads each running a tree therefore
    /// hand out the same integers, which is harmless — a key is only ever
    /// compared against the entries of the one root that painted them.
    ///
    /// Starts at `1` so `0` stays available as "no overlay" for a consumer that
    /// wants a niche-free sentinel.
    static NEXT_OVERLAY_KEY: Cell<u64> = const { Cell::new(1) };

    /// The entries registered so far in the paint pass currently running on this
    /// thread — written by
    /// [`PaintCtx::register_overlay`](crate::widget::PaintCtx::register_overlay)
    /// and drained by [`RenderRoot::paint`](crate::app::RenderRoot::paint).
    ///
    /// A side channel for the *routing* reason
    /// [`EventCtx::set_cursor`](crate::event::EventCtx::set_cursor)'s slot is
    /// one, not for a missing-handle reason: an entry means nothing to any
    /// container between the owner and the root, so bubbling it pod by pod would
    /// widen every container's paint absorb to carry a payload no container
    /// reads. Unlike the cursor this is a `Vec` rather than a last-writer-wins
    /// slot — any number of owners may float a surface in one pass, and
    /// registration order is what orders them within a band.
    ///
    /// **Pass-scoped**: cleared when a pass opens and drained when it closes, so
    /// an entry registered by a widget painted with no root above it (a leaf
    /// unit test) is dropped by the next pass's clear rather than leaking into
    /// it.
    static OVERLAY_REGISTRY: Cell<Vec<OverlayEntry>> = const { Cell::new(Vec::new()) };

    /// Whether an overlay registration pass is open on this thread — owned by
    /// [`OverlayPaintPass`] alone (see [`PassBracket`]).
    static OVERLAY_PASS_OPEN: Cell<bool> = const { Cell::new(false) };
}

/// An overlay owner's identity, allocated once by the owner and quoted back to
/// it by every [`InputEvent::Overlay`](crate::event::InputEvent::Overlay) the
/// root routes into its surface.
///
/// Stable for the owner's lifetime: an owner allocates one key when it is built
/// and stores it, re-registering under the same key every paint. The root
/// therefore never has to recognise an owner by position, and an owner that
/// floats two surfaces simply holds two keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OverlayKey(u64);

impl OverlayKey {
    /// Allocate a fresh key, distinct from every other key handed out on this
    /// thread.
    ///
    /// An owner calls this **once**, when it is built, and stores the result —
    /// calling it per frame would hand the root a new identity every paint and
    /// break the routing it exists to enable.
    pub fn next() -> Self {
        OverlayKey(NEXT_OVERLAY_KEY.with(|next| {
            let id = next.get();
            next.set(id.wrapping_add(1));
            id
        }))
    }

    /// The underlying integer, for a consumer that needs to key a map by it.
    /// Diagnostic and bookkeeping only — never derive routing from the *value*
    /// (allocation order is not a contract).
    pub fn value(self) -> u64 {
        self.0
    }
}

/// Which z-band a registered surface paints and hit-tests in.
///
/// Two bands, ordered `Floating` **below** `Tooltip`: a tooltip explaining a
/// menu item must never be painted under the menu, and — because hit testing
/// walks the bands in reverse — must never steal the pointer from it either
/// (a tooltip is normally registered [`OverlayInput::Transparent`] anyway).
/// Registration order breaks ties *within* a band; the band itself always wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OverlayBand {
    /// Menus, popovers, dropdowns, selection toolbars — anything the user can
    /// interact with. Painted first, hit-tested last.
    Floating,
    /// Tooltips and other explanatory chrome that must sit above everything
    /// else. Painted last, hit-tested first.
    Tooltip,
}

/// Whether a registered surface takes pointer input at all.
///
/// [`OverlayInput::Transparent`] is the rule egui's `Order::Tooltip` encodes: a
/// surface that is painted above the app but never hit-tested, so the pointer
/// passes straight through it to whatever the main tree has underneath. A
/// transparent entry is skipped by the root's pre-pass entirely — it receives no
/// [`InputEvent::Overlay`](crate::event::InputEvent::Overlay), including no
/// [`OverlayEventKind::OutsideDown`](crate::event::OverlayEventKind::OutsideDown).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OverlayInput {
    /// The surface hit-tests: a pointer inside its rect is routed to its owner
    /// and never reaches the main tree.
    Interactive,
    /// The surface is painted but never hit-tested — pointer input passes
    /// through as though it were not there.
    Transparent,
}

/// What a registered surface wants to hear about a press that landed on
/// *nothing* floated — the light-dismiss policy, a per-surface boolean
/// everywhere this pattern exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OutsideTap {
    /// Say nothing. A surface that dismisses some other way (a menu closed by
    /// its own item, a toolbar that follows a selection) wants this.
    Ignore,
    /// Deliver
    /// [`OverlayEventKind::OutsideDown`](crate::event::OverlayEventKind::OutsideDown)
    /// to the owner on a primary press outside every registered rect.
    Notify {
        /// Whether the press is **consumed** by the notification.
        ///
        /// `true` is the modal light-dismiss shape: the press closes the surface
        /// and the main tree never sees it, so the tap that dismisses a menu
        /// does not also activate the button under it. `false` is the
        /// pass-through shape: the owner is told, *and* the press continues into
        /// the main tree as usual.
        consume: bool,
    },
}

/// The shared handle an owner and the root address one floated pod through.
///
/// `Rc<RefCell<_>>` rather than a borrow because the two sides run in different
/// passes: the owner keeps one clone for the layout/event side, the registration
/// carries the other so the root can paint it after the owner's own paint has
/// returned. The root never *retains* a clone past the paint it was registered
/// for (see [`OverlayHit`]), so the pod's lifetime stays the owner's.
pub type OverlayPod = Rc<RefCell<ChildPod>>;

/// One floated surface, registered by its owner for the paint pass in progress.
///
/// Every field is a D7 rule the root reads back: `band` and registration order
/// decide paint and hit order, `input` decides whether the surface is hit-tested
/// at all, `outside_tap` decides what a press elsewhere delivers, and
/// `window_rect` is both where the pod is painted and what the root hit-tests.
#[derive(Clone)]
pub struct OverlayEntry {
    /// The owner's identity, quoted back on every routed
    /// [`InputEvent::Overlay`](crate::event::InputEvent::Overlay) so the owner
    /// recognises its own surface's input and every other widget ignores it.
    pub key: OverlayKey,
    /// Which z-band the surface paints and hit-tests in (`Floating` below
    /// `Tooltip`).
    pub band: OverlayBand,
    /// Whether the surface takes pointer input or is painted through.
    pub input: OverlayInput,
    /// What a primary press outside every registered rect delivers here.
    pub outside_tap: OutsideTap,
    /// Where the surface sits, in **absolute logical window space** — the one
    /// coordinate space both the root's paint origin and the root's hit test use.
    /// An owner computes it in `paint` from
    /// [`PaintCtx::origin`](crate::widget::PaintCtx::origin), the only absolute
    /// anchor a widget has, which is what makes an anchored surface follow its
    /// owner with no subscription of any kind.
    pub window_rect: Rect,
    /// The pod the root paints. The owner holds the other clone and lays this
    /// out itself, against the window size; the owner must **not** paint it —
    /// painting it as well would draw the surface twice, once clipped in place
    /// and once floated.
    ///
    /// The root paints it with `window_rect.origin()` as the absolute origin, and
    /// [`ChildPod::paint_child`](crate::widget::ChildPod::paint_child) then adds
    /// the pod's **own** origin on top as it does for any child — so leave that at
    /// [`Point::ZERO`](kurbo::Point::ZERO) unless you mean an offset the hit test
    /// will not know about (routing tests `window_rect` alone).
    pub pod: OverlayPod,
}

impl fmt::Debug for OverlayEntry {
    /// Prints every routing field and elides the pod: a [`ChildPod`] is not
    /// [`Debug`], and what a diagnostic wants from an entry is where it sits and
    /// how it routes, not what it contains.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverlayEntry")
            .field("key", &self.key)
            .field("band", &self.band)
            .field("input", &self.input)
            .field("outside_tap", &self.outside_tap)
            .field("window_rect", &self.window_rect)
            .field("pod", &"<pod>")
            .finish()
    }
}

/// What the root retains from an [`OverlayEntry`] to route the *next* pass's
/// input with: everything except the pod.
///
/// Dropping the pod handle is the point. The root holds these between paints, so
/// retaining an [`OverlayPod`] here would make the root a co-owner of a pod whose
/// owner may have been unmounted since, and would invite a borrow held across a
/// pass boundary. Routing needs none of it: the root decides *which key* the
/// event belongs to and broadcasts that key, and the owner — which does hold the
/// pod — forwards it in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayHit {
    /// The owner's identity, broadcast with the routed event.
    pub key: OverlayKey,
    /// The band, which orders the hit test (`Tooltip` tested before `Floating`).
    pub band: OverlayBand,
    /// Whether this entry is hit-tested at all.
    pub input: OverlayInput,
    /// What a press outside every rect delivers here.
    pub outside_tap: OutsideTap,
    /// The absolute logical rect the hit test runs against.
    pub window_rect: Rect,
}

impl OverlayHit {
    /// Take the routing half of `entry`, leaving the pod behind.
    pub(crate) fn of(entry: &OverlayEntry) -> Self {
        Self {
            key: entry.key,
            band: entry.band,
            input: entry.input,
            outside_tap: entry.outside_tap,
            window_rect: entry.window_rect,
        }
    }

    /// Whether `point` (absolute logical window space) lies inside this entry.
    ///
    /// Half-open on the far edges, exactly like
    /// [`ChildPod::contains`](crate::widget::ChildPod::contains), so two
    /// surfaces sharing an edge cannot both claim the same pixel.
    pub(crate) fn contains(&self, point: kurbo::Point) -> bool {
        point.x >= self.window_rect.x0
            && point.x < self.window_rect.x1
            && point.y >= self.window_rect.y0
            && point.y < self.window_rect.y1
    }
}

/// The open/close bracket around one overlay registration pass.
///
/// [`RenderRoot::paint`](crate::app::RenderRoot::paint) holds one for the length
/// of the pass: entering clears the registry (so nothing from a previous pass —
/// or from a widget painted with no root above it — can survive into this one),
/// [`OverlayPaintPass::take`] drains what this pass registered, and `Drop` hands
/// an enclosing pass its own in-progress registrations back.
///
/// The mechanism is [`PassBracket`]'s, shared with the event pass's request
/// slots; only the channel and the open flag differ.
pub(crate) struct OverlayPaintPass(PassBracket<Vec<OverlayEntry>>);

impl OverlayPaintPass {
    /// Open an overlay registration pass, starting from "nobody has registered
    /// anything".
    pub(crate) fn enter() -> Self {
        OverlayPaintPass(PassBracket::enter(&OVERLAY_REGISTRY, &OVERLAY_PASS_OPEN))
    }

    /// Take what *this* pass registered, in registration order, leaving the
    /// registry empty.
    pub(crate) fn take(&self) -> Vec<OverlayEntry> {
        self.0.take()
    }
}

/// Record `entry` in the pass currently painting on this thread.
///
/// The implementation behind
/// [`PaintCtx::register_overlay`](crate::widget::PaintCtx::register_overlay),
/// which is the API an owner calls; this is the module-private half so the slot
/// stays owned here.
pub(crate) fn register(entry: OverlayEntry) {
    OVERLAY_REGISTRY.with(|registry| {
        // Move the `Vec` out, push, move it back: a `Cell` rather than a
        // `RefCell` because the slot is bracketed by the shared [`PassBracket`]
        // machinery, which is written against `Cell`'s `take`/`set`. The move is
        // three words; the allocation is reused across the whole pass.
        let mut entries = registry.take();
        entries.push(entry);
        registry.set(entries);
    });
}

/// Sort `entries` into paint order in place: `Floating` first, then `Tooltip`,
/// preserving registration order within each band.
///
/// A **stable** sort, which is the whole rule — the band is the only key, so
/// stability is what makes "later registration paints above earlier" true within
/// a band. Reversing the result gives the hit-test order (topmost first).
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
            window_rect: rect,
            pod: Rc::new(RefCell::new(ChildPod::new(Box::new(StubWidget)))),
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
