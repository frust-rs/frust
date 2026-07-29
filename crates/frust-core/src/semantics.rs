//! Layer 2: the semantics seam — a pull-based accessibility tree pass.
//!
//! Each widget *optionally* contributes an [`accesskit::Node`] describing its
//! role, label, and state ([`Widget::semantics`](crate::widget::Widget::semantics),
//! defaulted to a no-op so no existing widget is affected). The collection pass
//! mirrors paint, not the arena tree-walk: a container's children are
//! [`ChildPod`](crate::widget::ChildPod)s outside the arena, so — exactly like
//! [`ChildPod::paint_child`](crate::widget::ChildPod::paint_child) threads an
//! absolute origin down — [`ChildPod::semantics_child`](crate::widget::ChildPod::semantics_child)
//! translates the current absolute origin into the child's space and recurses.
//!
//! [`RenderRoot::semantics`](crate::app::RenderRoot::semantics) drives it *after*
//! layout (so bounds are valid) and returns a [`SemanticsUpdate`]: a flat
//! `(NodeId, Node)` list plus the root id and the focused node id, ready for a
//! platform adapter (`accesskit_*`) to consume. This crate owns **no** platform
//! wiring or per-frame scheduling — those live in each platform shell.
//!
//! # Contributing a node
//!
//! A leaf widget calls [`SemanticsCtx::push_node`] with its role and a closure
//! that sets label/state; bounds are filled from the current absolute
//! origin/size automatically. A widget that also has semantics-visible children
//! (e.g. a scroll view) uses [`SemanticsCtx::push_container`], whose `visit`
//! closure recurses into the children (via `semantics_child`) so they become the
//! node's accesskit children. A *transparent* container (Flex/Stack/Padding/…)
//! contributes no node of its own — it just calls `semantics_child` for each
//! child, so their nodes attach to whatever encloses the container.

use std::num::NonZeroU64;

use accesskit::{Node, NodeId, Rect as AccessRect, Role};
use kurbo::{Point, Size, Vec2};

/// The reserved [`NodeId`] of the synthetic window/root node
/// ([`RenderRoot::semantics`](crate::app::RenderRoot::semantics) always roots the
/// tree here). It is a fixed constant — the one id never drawn from the
/// per-`ChildPod` allocator — so a platform adapter can treat "the root" as a
/// stable anchor across every frame. Pod-derived ids start well
/// above it (see [`SemanticsCtx::alloc_base`]).
pub const ROOT_NODE_ID: NodeId = NodeId(1);

/// Number of low bits of a composed [`NodeId`] reserved for a widget's per-pod
/// sub-node *slot* ordinal.
///
/// A [`ChildPod`](crate::widget::ChildPod)'s stable base id occupies the high
/// bits; the low [`SLOT_BITS`] bits distinguish the (at most `2^SLOT_BITS`)
/// nodes a single widget contributes for that pod — its own node is slot `0`,
/// and any extra nodes it pushes (navbar items, list rows, …) take slots
/// `1, 2, …` in a stable push order. This keeps every node id stable across
/// frames (the base survives the pod's lifetime, including keyed relocation)
/// while staying collision-free: distinct bases never overlap as long as a
/// single widget contributes fewer than `2^SLOT_BITS` nodes and the base fits
/// the remaining `64 - SLOT_BITS` bits.
const SLOT_BITS: u32 = 16;

/// Compose a stable [`NodeId`] from a pod's `base` id and a per-pod sub-node
/// `slot` ordinal (see [`SLOT_BITS`]).
///
/// `base` is a monotonically-allocated, never-reused per-pod id (`>= 2`, so the
/// composed value never collides with the reserved [`ROOT_NODE_ID`]); `slot`
/// distinguishes the nodes one widget contributes for that pod. The mapping is
/// injective over `(base, slot)` while `base < 2^(64 - SLOT_BITS)`, which a
/// `NonZeroU64` allocator exhausts only after ~2.8e14 pods.
pub(crate) fn compose_node_id(base: NonZeroU64, slot: u16) -> NodeId {
    debug_assert!(
        base.get() < (1u64 << (64 - SLOT_BITS)),
        "semantics base id {} overflows the {}-bit base field",
        base.get(),
        64 - SLOT_BITS
    );
    NodeId((base.get() << SLOT_BITS) | slot as u64)
}

/// A collected accessibility tree produced by
/// [`RenderRoot::semantics`](crate::app::RenderRoot::semantics).
///
/// `nodes` is the flat `(NodeId, Node)` map every accesskit platform adapter
/// consumes; `root` is the id of the enclosing window node (always present, even
/// for an empty tree); `focus` is the node a focused widget claimed via
/// [`SemanticsCtx::set_focused`], or `None` when nothing in the tree is focused
/// (a platform adapter defaults the platform focus to `root` in that case).
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticsUpdate {
    /// The flat node map, root first, then each contributed node in the order it
    /// was collected (pre-order, mirroring paint).
    pub nodes: Vec<(NodeId, Node)>,
    /// The id of the root (window) node.
    pub root: NodeId,
    /// The focused node, if any widget claimed focus this pass.
    pub focus: Option<NodeId>,
}

impl SemanticsUpdate {
    /// The node a platform adapter should report as focused — the focused widget's
    /// node, or [`root`](SemanticsUpdate::root) when nothing in the tree is
    /// focused.
    ///
    /// accesskit's `TreeUpdate::focus` is a non-optional [`NodeId`]: an adapter
    /// must always name *some* focus target, and the window root is the
    /// conventional fallback. This is the value a shell feeds
    /// straight into the adapter, versus reading [`focus`](SemanticsUpdate::focus)
    /// when it needs to distinguish "root, because focused" from "root, because
    /// nothing is focused".
    pub fn focus_id(&self) -> NodeId {
        self.focus.unwrap_or(self.root)
    }
}

/// Collects [`accesskit::Node`]s during a semantics pass, tracking the current
/// absolute origin/size (mirroring paint's origin threading) and the
/// parent/child structure.
///
/// Node ids are **stable across passes**: each node's id is
/// composed from the owning [`ChildPod`](crate::widget::ChildPod)'s persistent
/// base id (assigned on first visit from a monotonic, never-reused
/// [`RenderRoot`](crate::app::RenderRoot) allocator and threaded in through
/// [`SemanticsCtx::new`]) and a per-pod sub-node slot (see
/// [`compose_node_id`]) — so the same widget keeps the same id every frame,
/// including across a keyed reorder that relocates its pod. accesskit
/// `TreeUpdate`s require stable ids for assistive-technology focus continuity.
/// Bounds are derived from the current absolute origin/size when a node is
/// pushed, so a widget never computes its own absolute rect.
pub struct SemanticsCtx {
    /// Absolute origin of the widget currently being visited.
    origin: Point,
    /// Size of the widget currently being visited.
    size: Size,
    /// Monotonic *fallback* node-id counter, used only for a node pushed with no
    /// current pod base (the direct-`push_node` unit-test path). Real tree walks
    /// always compose ids from a pod base — see [`SemanticsCtx::next_node_id`].
    next_id: u64,
    /// The persistent per-pod base-id allocator's next value, seeded by
    /// [`RenderRoot`](crate::app::RenderRoot) from its cross-pass high-water mark
    /// and read back via [`SemanticsCtx::next_base`] after the pass so newly-seen
    /// pods never reuse an already-assigned base.
    next_base: u64,
    /// The base id of the pod currently being visited (set by
    /// [`SemanticsCtx::descend_into_pod`]); `None` at the window-root level and in
    /// direct `push_node` unit tests.
    current_base: Option<NonZeroU64>,
    /// The next sub-node slot ordinal for the current pod — bumped by each
    /// [`SemanticsCtx::next_node_id`] so a widget contributing several nodes gets
    /// stable, distinct ids.
    current_slot: u16,
    /// The flat node map accumulated so far.
    nodes: Vec<(NodeId, Node)>,
    /// A stack of child-id collection frames: the top frame gathers the ids of
    /// nodes contributed at the current nesting level, so a container can set
    /// them as its node's children after recursing.
    frames: Vec<Vec<NodeId>>,
    /// The focused node, recorded by [`SemanticsCtx::set_focused`].
    focus: Option<NodeId>,
}

impl SemanticsCtx {
    /// Create a context for a pass over a `window_size`-sized root, positioned at
    /// the origin, seeding the persistent per-pod base allocator at `next_base`
    /// (the caller's cross-pass high-water mark). One (root-level) child frame is
    /// open.
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

    /// The absolute origin of the widget currently being visited.
    pub fn origin(&self) -> Point {
        self.origin
    }

    /// The size of the widget currently being visited.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Allocate the next persistent per-pod base id, advancing the allocator.
    ///
    /// A [`ChildPod`](crate::widget::ChildPod) calls this on its *first* semantics
    /// visit and caches the result for its whole lifetime; the
    /// [`RenderRoot`](crate::app::RenderRoot) reads the final value back via
    /// [`SemanticsCtx::next_base`] so the next pass never reuses it.
    pub(crate) fn alloc_base(&mut self) -> NonZeroU64 {
        let id = self.next_base;
        self.next_base += 1;
        NonZeroU64::new(id).expect("base allocator is seeded >= 2, never zero")
    }

    /// The allocator's next value after the pass — the caller's new cross-pass
    /// high-water mark (see [`SemanticsCtx::alloc_base`]).
    pub(crate) fn next_base(&self) -> u64 {
        self.next_base
    }

    /// Allocate the next sequential *fallback* node id (no pod base in scope).
    fn alloc_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    /// The [`NodeId`] for the next node the current widget contributes: composed
    /// from the current pod base and its running slot ([`compose_node_id`]) when a
    /// pod is in scope, else the sequential fallback (direct-`push_node` tests).
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

    /// The accesskit bounds of the widget currently being visited (absolute,
    /// top-down y), derived from the current origin/size.
    fn bounds(&self) -> AccessRect {
        AccessRect {
            x0: self.origin.x,
            y0: self.origin.y,
            x1: self.origin.x + self.size.width,
            y1: self.origin.y + self.size.height,
        }
    }

    /// Contribute a **leaf** semantics node for the current widget.
    ///
    /// Allocates a node id, creates an [`accesskit::Node`] of `role` with its
    /// bounds set from the current absolute origin/size, lets `build` set the
    /// label/state, records it as a child of the enclosing node, and appends it
    /// to the flat map. Returns the allocated id (e.g. so a widget can
    /// [`set_focused`](SemanticsCtx::set_focused) it).
    pub fn push_node(&mut self, role: Role, build: impl FnOnce(&mut Node)) -> NodeId {
        let id = self.next_node_id();
        let mut node = Node::new(role);
        node.set_bounds(self.bounds());
        build(&mut node);
        self.attach(id, node);
        id
    }

    /// Contribute a **container** semantics node whose accesskit children are the
    /// nodes contributed during `visit`.
    ///
    /// Like [`push_node`](SemanticsCtx::push_node) but opens a fresh child frame,
    /// runs `visit` (which recurses into the container's
    /// [`ChildPod`](crate::widget::ChildPod)s via
    /// [`semantics_child`](crate::widget::ChildPod::semantics_child)), then sets
    /// the collected ids as this node's [`Node::set_children`]. The container node
    /// is appended to the flat map *after* its children (post-order), but is still
    /// registered as a child of its own parent frame.
    pub fn push_container(
        &mut self,
        role: Role,
        build: impl FnOnce(&mut Node),
        visit: impl FnOnce(&mut SemanticsCtx),
    ) -> NodeId {
        let id = self.next_node_id();
        self.push_container_inner(id, role, build, visit)
    }

    /// Like [`push_container`](SemanticsCtx::push_container) but with an explicit,
    /// caller-chosen `id` — used for the reserved [`ROOT_NODE_ID`] window node,
    /// which is not owned by any pod.
    pub(crate) fn push_container_with_id(
        &mut self,
        id: NodeId,
        role: Role,
        build: impl FnOnce(&mut Node),
        visit: impl FnOnce(&mut SemanticsCtx),
    ) -> NodeId {
        self.push_container_inner(id, role, build, visit)
    }

    /// Shared container body: build the node, open a fresh child frame, run
    /// `visit`, then set the collected ids as this node's children.
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

    /// Register `id` as a child of the enclosing frame and store its node.
    fn attach(&mut self, id: NodeId, node: Node) {
        self.frames
            .last_mut()
            .expect("a child frame is always open during a pass")
            .push(id);
        self.nodes.push((id, node));
    }

    /// Record that the node `id` holds input focus (accesskit tracks focus at the
    /// tree level, not per-node). The last widget to call this in a pass wins.
    pub fn set_focused(&mut self, id: NodeId) {
        self.focus = Some(id);
    }

    /// Run `f` with the current geometry translated into a child's space:
    /// `child_offset` is the child's origin *relative to the current origin*
    /// (matching [`ChildPod::paint_child`](crate::widget::ChildPod::paint_child)'s
    /// `ctx.origin() + pod.origin` absolute-origin rule), and `child_size` its
    /// resolved size. The previous geometry is restored afterward.
    ///
    /// Geometry-only sibling of [`descend_into_pod`](SemanticsCtx::descend_into_pod)
    /// (which also enters a pod's stable-id scope); real tree walks always go
    /// through the pod variant, so this is retained only for the direct-`push_node`
    /// unit tests that exercise origin threading without a pod.
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

    /// Enter a [`ChildPod`](crate::widget::ChildPod)'s geometry **and** its stable
    /// id scope: like [`descend`](SemanticsCtx::descend) it translates into the
    /// child's absolute space, and additionally makes `base` the current pod base
    /// (resetting the sub-node slot counter) so nodes the child contributes get
    /// stable [`compose_node_id`]-composed ids. The previous geometry, base, and
    /// slot are all restored afterward.
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

    /// Finish the pass, returning the collected update rooted at `root`.
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
