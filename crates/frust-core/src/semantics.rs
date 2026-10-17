//! The semantics seam: a pull-based accessibility tree pass.
//!
//! A widget optionally contributes an [`accesskit::Node`] from
//! [`Widget::semantics`](crate::widget::Widget::semantics). The pass mirrors paint:
//! [`ChildPod::semantics_child`](crate::widget::ChildPod::semantics_child) threads the
//! absolute origin into each child, and
//! [`RenderRoot::semantics`](crate::app::RenderRoot::semantics) runs it after layout
//! and returns a [`SemanticsUpdate`] for a shell's `accesskit_*` adapter.
//!
//! A leaf calls [`SemanticsCtx::push_node`]; a container with semantic children
//! calls [`SemanticsCtx::push_container`] and recurses in `visit`; a transparent
//! container (Flex, Stack, Padding) only calls `semantics_child` on each child.

use std::num::NonZeroU64;

use accesskit::{Node, NodeId, Rect as AccessRect, Role};
use kurbo::{Point, Size, Vec2};

/// The fixed id of the synthetic window node every tree is rooted at; never
/// drawn from the per-pod allocator, so it is stable across frames.
pub const ROOT_NODE_ID: NodeId = NodeId(1);

/// Low bits of a composed [`NodeId`] holding the per-pod slot: a widget's own
/// node is slot `0`, extra nodes it pushes take `1, 2, …` in push order. Ids
/// stay stable and collision-free while one widget pushes fewer than `2^16`.
const SLOT_BITS: u32 = 16;

/// `(base << SLOT_BITS) | slot`. `base` is a never-reused per-pod id `>= 2`, so
/// no composed id equals [`ROOT_NODE_ID`]; injective while `base < 2^48`.
pub(crate) fn compose_node_id(base: NonZeroU64, slot: u16) -> NodeId {
    debug_assert!(
        base.get() < (1u64 << (64 - SLOT_BITS)),
        "semantics base id {} overflows the {}-bit base field",
        base.get(),
        64 - SLOT_BITS
    );
    NodeId((base.get() << SLOT_BITS) | slot as u64)
}

/// A collected accessibility tree from
/// [`RenderRoot::semantics`](crate::app::RenderRoot::semantics).
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticsUpdate {
    /// The flat node map in collection order: each container follows its
    /// children (post-order), so the root window node comes last.
    pub nodes: Vec<(NodeId, Node)>,
    /// The window node, present even for an empty tree.
    pub root: NodeId,
    /// The node a widget claimed with [`SemanticsCtx::set_focused`], if any.
    pub focus: Option<NodeId>,
}

impl SemanticsUpdate {
    /// The focus to hand an adapter: [`focus`](Self::focus), else
    /// [`root`](Self::root), since accesskit's `TreeUpdate::focus` is not optional.
    pub fn focus_id(&self) -> NodeId {
        self.focus.unwrap_or(self.root)
    }
}

/// Collects nodes during a semantics pass, threading the absolute origin/size
/// and the parent/child structure.
///
/// Node ids are stable across passes: each is composed from the owning pod's
/// persistent base id and a per-pod slot, so a widget keeps its id every frame,
/// keyed reorders included, as assistive-technology focus continuity requires.
pub struct SemanticsCtx {
    origin: Point,
    size: Size,
    /// Fallback id counter for a node pushed with no pod base in scope (the
    /// direct-`push_node` unit tests only).
    next_id: u64,
    /// The base allocator's next value; the root reads it back after the pass so
    /// a base is never reused.
    next_base: u64,
    /// The pod being visited; `None` at the window-root level.
    current_base: Option<NonZeroU64>,
    current_slot: u16,
    nodes: Vec<(NodeId, Node)>,
    /// One frame per nesting level, gathering the child ids its container sets.
    frames: Vec<Vec<NodeId>>,
    focus: Option<NodeId>,
}

impl SemanticsCtx {
    /// A pass over a `window_size` root with the base allocator at `next_base`;
    /// one root-level child frame is open.
    pub(crate) fn new(window_size: Size, next_base: u64) -> Self {
        Self {
            origin: Point::ZERO,
            size: window_size,
            next_id: 0,
            next_base,
            current_base: None,
            current_slot: 0,
            nodes: Vec::new(),
            frames: vec![Vec::new()],
            focus: None,
        }
    }

    /// The absolute origin of the widget being visited.
    pub fn origin(&self) -> Point {
        self.origin
    }

    /// The size of the widget being visited.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Allocate a persistent pod base id; a pod calls this on its first visit and
    /// caches the result for its lifetime.
    pub(crate) fn alloc_base(&mut self) -> NonZeroU64 {
        let id = self.next_base;
        self.next_base += 1;
        NonZeroU64::new(id).expect("base allocator is seeded >= 2, never zero")
    }

    pub(crate) fn next_base(&self) -> u64 {
        self.next_base
    }

    fn alloc_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    /// The current pod's next composed id, else the fallback counter.
    fn next_node_id(&mut self) -> NodeId {
        match self.current_base {
            Some(base) => {
                let slot = self.current_slot;
                self.current_slot = self
                    .current_slot
                    .checked_add(1)
                    .expect("a single widget contributes fewer than 2^16 nodes per pod");
                compose_node_id(base, slot)
            }
            None => self.alloc_id(),
        }
    }

    /// Absolute, top-down bounds of the widget being visited.
    fn bounds(&self) -> AccessRect {
        AccessRect {
            x0: self.origin.x,
            y0: self.origin.y,
            x1: self.origin.x + self.size.width,
            y1: self.origin.y + self.size.height,
        }
    }

    /// Contribute a leaf node of `role` with bounds from the current geometry;
    /// `build` sets label and state. Returns its id.
    pub fn push_node(&mut self, role: Role, build: impl FnOnce(&mut Node)) -> NodeId {
        let id = self.next_node_id();
        let mut node = Node::new(role);
        node.set_bounds(self.bounds());
        build(&mut node);
        self.attach(id, node);
        id
    }

    /// Contribute a container node whose children are the nodes contributed
    /// during `visit`; it is stored after them (post-order).
    pub fn push_container(
        &mut self,
        role: Role,
        build: impl FnOnce(&mut Node),
        visit: impl FnOnce(&mut SemanticsCtx),
    ) -> NodeId {
        let id = self.next_node_id();
        self.push_container_inner(id, role, build, visit)
    }

    /// [`push_container`](Self::push_container) under a caller-chosen `id` (the
    /// window node's [`ROOT_NODE_ID`]).
    pub(crate) fn push_container_with_id(
        &mut self,
        id: NodeId,
        role: Role,
        build: impl FnOnce(&mut Node),
        visit: impl FnOnce(&mut SemanticsCtx),
    ) -> NodeId {
        self.push_container_inner(id, role, build, visit)
    }

    fn push_container_inner(
        &mut self,
        id: NodeId,
        role: Role,
        build: impl FnOnce(&mut Node),
        visit: impl FnOnce(&mut SemanticsCtx),
    ) -> NodeId {
        let mut node = Node::new(role);
        node.set_bounds(self.bounds());
        build(&mut node);
        self.frames.push(Vec::new());
        visit(self);
        let children = self.frames.pop().expect("container frame was just pushed");
        node.set_children(children);
        self.attach(id, node);
        id
    }

    fn attach(&mut self, id: NodeId, node: Node) {
        self.frames
            .last_mut()
            .expect("a child frame is always open during a pass")
            .push(id);
        self.nodes.push((id, node));
    }

    /// Record `id` as the focused node; the last call in a pass wins.
    pub fn set_focused(&mut self, id: NodeId) {
        self.focus = Some(id);
    }

    /// Run `f` translated by `child_offset` into a `child_size` child, restoring
    /// the geometry after. Geometry only; for the unit tests without a pod.
    #[cfg(test)]
    pub(crate) fn descend(
        &mut self,
        child_offset: Vec2,
        child_size: Size,
        f: impl FnOnce(&mut Self),
    ) {
        let saved_origin = self.origin;
        let saved_size = self.size;
        self.origin = saved_origin + child_offset;
        self.size = child_size;
        f(self);
        self.origin = saved_origin;
        self.size = saved_size;
    }

    /// Like `descend`, and also enter pod `base`'s id scope with the slot reset;
    /// geometry, base and slot are all restored after.
    pub(crate) fn descend_into_pod(
        &mut self,
        base: NonZeroU64,
        child_offset: Vec2,
        child_size: Size,
        f: impl FnOnce(&mut Self),
    ) {
        let saved_origin = self.origin;
        let saved_size = self.size;
        let saved_base = self.current_base;
        let saved_slot = self.current_slot;
        self.origin = saved_origin + child_offset;
        self.size = child_size;
        self.current_base = Some(base);
        self.current_slot = 0;
        f(self);
        self.origin = saved_origin;
        self.size = saved_size;
        self.current_base = saved_base;
        self.current_slot = saved_slot;
    }

    pub(crate) fn finish(self, root: NodeId) -> SemanticsUpdate {
        SemanticsUpdate {
            nodes: self.nodes,
            root,
            focus: self.focus,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The base seed a `RenderRoot` passes on the first pass (ids 0 and 1 are
    /// reserved: 0 keeps `NonZeroU64` valid, 1 is the window root).
    const BASE_SEED: u64 = 2;

    #[test]
    fn alloc_ids_are_sequential_and_deterministic() {
        let mut ctx = SemanticsCtx::new(Size::new(100.0, 100.0), BASE_SEED);
        assert_eq!(ctx.alloc_id(), NodeId(0));
        assert_eq!(ctx.alloc_id(), NodeId(1));
        assert_eq!(ctx.alloc_id(), NodeId(2));
    }

    #[test]
    fn push_node_sets_absolute_bounds_from_origin_and_size() {
        let mut ctx = SemanticsCtx::new(Size::new(200.0, 200.0), BASE_SEED);
        // Descend into a child placed at (10, 20) sized 30x40.
        ctx.descend(Vec2::new(10.0, 20.0), Size::new(30.0, 40.0), |ctx| {
            let id = ctx.push_node(Role::Label, |node| node.set_label("hi"));
            assert_eq!(id, NodeId(0));
        });
        let (_, node) = &ctx.nodes[0];
        assert_eq!(
            node.bounds(),
            Some(AccessRect {
                x0: 10.0,
                y0: 20.0,
                x1: 40.0,
                y1: 60.0,
            })
        );
        assert_eq!(node.label(), Some("hi"));
        assert_eq!(node.role(), Role::Label);
    }

    #[test]
    fn descend_composes_nested_origins() {
        // A grandchild's absolute origin is the sum of the whole ancestor chain,
        // mirroring paint_child's `ctx.origin() + pod.origin`.
        let mut ctx = SemanticsCtx::new(Size::new(500.0, 500.0), BASE_SEED);
        ctx.descend(Vec2::new(100.0, 200.0), Size::new(300.0, 300.0), |ctx| {
            ctx.descend(Vec2::new(5.0, 7.0), Size::new(10.0, 10.0), |ctx| {
                ctx.push_node(Role::Button, |_| {});
            });
        });
        let (_, node) = &ctx.nodes[0];
        assert_eq!(
            node.bounds(),
            Some(AccessRect {
                x0: 105.0,
                y0: 207.0,
                x1: 115.0,
                y1: 217.0,
            })
        );
    }

    #[test]
    fn push_container_collects_children_and_restores_frame() {
        let mut ctx = SemanticsCtx::new(Size::new(100.0, 100.0), BASE_SEED);
        let container = ctx.push_container(
            Role::ScrollView,
            |_| {},
            |ctx| {
                ctx.push_node(Role::Label, |n| n.set_label("a"));
                ctx.push_node(Role::Label, |n| n.set_label("b"));
            },
        );
        // The container is the last node pushed (post-order); its children are the
        // two labels, allocated ids 1 and 2 (container reserved id 0).
        assert_eq!(container, NodeId(0));
        let update = ctx.finish(container);
        // Flat map order: label a (1), label b (2), then the container (0).
        assert_eq!(update.nodes.len(), 3);
        let container_node = &update
            .nodes
            .iter()
            .find(|(id, _)| *id == container)
            .unwrap()
            .1;
        assert_eq!(container_node.children(), &[NodeId(1), NodeId(2)]);
    }

    #[test]
    fn set_focused_records_the_focus_node() {
        let mut ctx = SemanticsCtx::new(Size::new(10.0, 10.0), BASE_SEED);
        let id = ctx.push_node(Role::TextInput, |_| {});
        ctx.set_focused(id);
        let update = ctx.finish(id);
        assert_eq!(update.focus, Some(id));
    }

    #[test]
    fn focus_id_defaults_to_root_when_unfocused() {
        let mut ctx = SemanticsCtx::new(Size::new(10.0, 10.0), BASE_SEED);
        let root = ctx.push_container(Role::Window, |_| {}, |_| {});
        let update = ctx.finish(root);
        assert!(update.focus.is_none());
        // With nothing focused, the adapter-facing focus id is the root itself.
        assert_eq!(update.focus_id(), update.root);
    }

    #[test]
    fn compose_node_id_is_collision_free_across_bases_and_slots() {
        // Distinct (base, slot) pairs must map to distinct ids, and one base's
        // slot range must never overlap the next base's — the property the
        // stable-id scheme relies on.
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for base in 2u64..40 {
            let base = NonZeroU64::new(base).unwrap();
            for slot in 0u16..1000 {
                let id = compose_node_id(base, slot);
                assert!(id != ROOT_NODE_ID, "never collides with the reserved root");
                assert!(seen.insert(id), "duplicate id for base={base} slot={slot}");
            }
        }
        // The max slot of one base sits strictly below the next base's slot 0.
        let a = NonZeroU64::new(2).unwrap();
        let b = NonZeroU64::new(3).unwrap();
        assert!(compose_node_id(a, u16::MAX).0 < compose_node_id(b, 0).0);
    }

    #[test]
    fn descend_into_pod_composes_stable_ids_and_restores_scope() {
        let mut ctx = SemanticsCtx::new(Size::new(100.0, 100.0), BASE_SEED);
        let base = ctx.alloc_base();
        assert_eq!(base.get(), BASE_SEED);
        // A widget contributing two nodes for its pod gets slots 0 and 1.
        let (first, second) = {
            let mut ids = (NodeId(0), NodeId(0));
            ctx.descend_into_pod(base, Vec2::ZERO, Size::new(10.0, 10.0), |ctx| {
                ids.0 = ctx.push_node(Role::Label, |_| {});
                ids.1 = ctx.push_node(Role::Label, |_| {});
            });
            ids
        };
        assert_eq!(first, compose_node_id(base, 0));
        assert_eq!(second, compose_node_id(base, 1));
        // After the pod scope closes, a fresh pod at the top level uses the
        // fallback sequential id (no base in scope) — proving the base/slot were
        // restored rather than leaking out.
        let outside = ctx.push_node(Role::Label, |_| {});
        assert_eq!(outside, NodeId(0));
    }
}
