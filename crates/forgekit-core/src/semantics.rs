//! Layer 2: the semantics seam — a pull-based accessibility tree pass (spec §9,
//! phase-6c D1).
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
//! wiring or per-frame scheduling — those are phase 6d.
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

use accesskit::{Node, NodeId, Rect as AccessRect, Role};
use kurbo::{Point, Size, Vec2};

/// A collected accessibility tree produced by
/// [`RenderRoot::semantics`](crate::app::RenderRoot::semantics).
///
/// `nodes` is the flat `(NodeId, Node)` map every accesskit platform adapter
/// consumes; `root` is the id of the enclosing window node (always present, even
/// for an empty tree); `focus` is the node a focused widget claimed via
/// [`SemanticsCtx::set_focused`], or `None` when nothing in the tree is focused
/// (a 6d adapter defaults the platform focus to `root` in that case).
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

/// Collects [`accesskit::Node`]s during a semantics pass, tracking the current
/// absolute origin/size (mirroring paint's origin threading) and the
/// parent/child structure.
///
/// Node ids are allocated sequentially and deterministically per pass, so the
/// same tree yields the same ids every collection. Bounds are derived from the
/// current absolute origin/size when a node is pushed, so a widget never
/// computes its own absolute rect.
pub struct SemanticsCtx {
    /// Absolute origin of the widget currently being visited.
    origin: Point,
    /// Size of the widget currently being visited.
    size: Size,
    /// Monotonic node-id counter (deterministic per pass).
    next_id: u64,
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
    /// the origin. One (root-level) child frame is open.
    pub(crate) fn new(window_size: Size) -> Self {
        Self {
            origin: Point::ZERO,
            size: window_size,
            next_id: 0,
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

    /// Allocate the next sequential node id.
    fn alloc_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
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
        let id = self.alloc_id();
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
        let id = self.alloc_id();
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

    #[test]
    fn alloc_ids_are_sequential_and_deterministic() {
        let mut ctx = SemanticsCtx::new(Size::new(100.0, 100.0));
        assert_eq!(ctx.alloc_id(), NodeId(0));
        assert_eq!(ctx.alloc_id(), NodeId(1));
        assert_eq!(ctx.alloc_id(), NodeId(2));
    }

    #[test]
    fn push_node_sets_absolute_bounds_from_origin_and_size() {
        let mut ctx = SemanticsCtx::new(Size::new(200.0, 200.0));
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
        let mut ctx = SemanticsCtx::new(Size::new(500.0, 500.0));
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
        let mut ctx = SemanticsCtx::new(Size::new(100.0, 100.0));
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
        let mut ctx = SemanticsCtx::new(Size::new(10.0, 10.0));
        let id = ctx.push_node(Role::TextInput, |_| {});
        ctx.set_focused(id);
        let update = ctx.finish(id);
        assert_eq!(update.focus, Some(id));
    }
}
