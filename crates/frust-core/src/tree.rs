//! The retained widget tree, backed by `tree_arena`.
//!
//! `tree_arena` gives O(1) access to any node with *simultaneous* mutable
//! access to a node's value and its children — the proven answer to the
//! borrow-checker fights a widget tree otherwise causes. All direct use of the
//! arena is confined to this module; the rest of the crate speaks in
//! [`WidgetId`]/[`WidgetPod`] terms.
//!
//! v0 is single-root (the app has one root widget), but the API is written in
//! terms of insert-into-list / find so container support drops in
//! without reshaping this layer.
//!
//! # Introspection
//!
//! [`WidgetTree::roots`] / [`WidgetTree::children`] / [`WidgetTree::inspect`]
//! make the arena enumerable for read-only tooling. `tree_arena` 0.2 does expose
//! `root_ids`/`child_ids`, but both iterate a `HashMap` in **arbitrary** order,
//! which is useless for an inspector that has to show siblings the way the app
//! declared them. The tree therefore keeps a parallel insertion-order index
//! (`root_order`/`child_order`) updated at the two mutation points that exist
//! ([`WidgetTree::insert_root`] and [`WidgetTree::insert_child`]) and uses it
//! only to *order* what the arena reports — the arena stays the single source of
//! truth for membership, so a stale index entry can never surface a dangling id.
//!
//! Containers own their children as [`ChildPod`]s rather than as arena nodes
//! (see that type's docs), so the arena itself holds little more than the root
//! pod. [`WidgetTree::inspect`] therefore walks *both*: a node's arena children
//! first, then the pods its widget hands to
//! [`Widget::visit_children`] — so the snapshot is the real retained hierarchy
//! rather than the arena's contents. Only `inspect` descends that way;
//! [`WidgetTree::children`] stays strictly arena-scoped, since it answers "what
//! does the arena hold under this id".
//!
//! The descent needs no cycle check: a pod owns its widget, ownership is a tree,
//! and the visitor only ever hands out `&ChildPod`s the widget itself owns (see
//! [`Widget::visit_children`]).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use kurbo::{Point, Rect, Size};
use tree_arena::{ArenaMut, ArenaRef, TreeArena};

use crate::view::{ChangeFlags, WidgetId};
use crate::widget::{ChildPod, Widget};

/// A retained widget plus its layout results and dirty state.
///
/// The "pod" wraps the boxed widget with the bookkeeping the framework owns:
/// where the widget was placed (`origin`), how big it is (`size`), and which
/// passes are pending (`flags`).
pub struct WidgetPod {
    id: WidgetId,
    widget: Box<dyn Widget>,
    origin: Point,
    size: Size,
    flags: ChangeFlags,
    /// The concrete widget type's [`core::any::type_name`], captured once at
    /// construction — a `&'static str` copy, so no per-frame cost.
    type_name: &'static str,
    /// An optional human name for tooling, `None` unless something calls
    /// [`WidgetPod::set_debug_label`].
    debug_label: Option<Cow<'static, str>>,
}

impl WidgetPod {
    /// The `type_name` recorded for a pod built from an already type-erased
    /// widget, where the concrete type is no longer nameable.
    pub const ERASED_TYPE_NAME: &'static str = "<erased dyn Widget>";

    /// Wrap a freshly built, already type-erased widget. It starts dirty for
    /// layout and paint.
    ///
    /// The concrete type is gone by the time the box arrives, so
    /// [`type_name`](WidgetPod::type_name) reports
    /// [`ERASED_TYPE_NAME`](WidgetPod::ERASED_TYPE_NAME). Prefer
    /// [`WidgetPod::new_typed`] where the concrete type is still in scope.
    pub fn new(id: WidgetId, widget: Box<dyn Widget>) -> Self {
        Self::with_type_name(id, widget, Self::ERASED_TYPE_NAME)
    }

    /// Wrap a freshly built widget whose concrete type is still in scope,
    /// recording its [`core::any::type_name`] for introspection.
    ///
    /// Pass the widget itself, never an already-boxed `Box<dyn Widget>`: the
    /// blanket `Widget for Box<dyn Widget>` impl would make that compile while
    /// recording the box's own type name and double-boxing the widget (which
    /// breaks the downcast back to the originating view's element type). Use
    /// [`WidgetPod::new`] for the erased case.
    pub fn new_typed<W: Widget>(id: WidgetId, widget: W) -> Self {
        Self::with_type_name(id, Box::new(widget), core::any::type_name::<W>())
    }

    fn with_type_name(id: WidgetId, widget: Box<dyn Widget>, type_name: &'static str) -> Self {
        Self {
            id,
            widget,
            origin: Point::ZERO,
            size: Size::ZERO,
            flags: ChangeFlags::LAYOUT | ChangeFlags::PAINT,
            type_name,
            debug_label: None,
        }
    }

    /// This pod's stable identity.
    pub fn id(&self) -> WidgetId {
        self.id
    }

    /// The concrete widget type's name, or
    /// [`ERASED_TYPE_NAME`](WidgetPod::ERASED_TYPE_NAME) when the pod was built
    /// from an already-erased box.
    ///
    /// Recorded at construction, which is sound here because a root pod's
    /// element type is fixed by `RenderRoot<State, V>`'s `V` and cannot swap
    /// under it. A [`ChildPod`], whose widget *can* be swapped by a rebuild,
    /// asks the live widget instead ([`Widget::type_name`]).
    ///
    /// Diagnostic only: `type_name`'s output is not a stable contract across
    /// compiler versions, so never parse or match on it.
    pub fn type_name(&self) -> &'static str {
        self.type_name
    }

    /// The human name attached for tooling, if any. `None` by default.
    pub fn debug_label(&self) -> Option<&str> {
        self.debug_label.as_deref()
    }

    /// Attach a human name for tooling (an inspector shows it beside the type
    /// name). Purely descriptive — nothing in the build/layout/paint path reads
    /// it.
    pub fn set_debug_label(&mut self, label: impl Into<Cow<'static, str>>) {
        self.debug_label = Some(label.into());
    }

    /// Drop any attached debug label.
    pub fn clear_debug_label(&mut self) {
        self.debug_label = None;
    }

    /// Shared access to the boxed widget.
    pub fn widget(&self) -> &dyn Widget {
        &*self.widget
    }

    /// Mutable access to the boxed widget.
    pub fn widget_mut(&mut self) -> &mut dyn Widget {
        &mut *self.widget
    }

    /// The widget's origin in its parent's coordinate space.
    pub fn origin(&self) -> Point {
        self.origin
    }

    /// The widget's resolved size (valid after a layout pass).
    pub fn size(&self) -> Size {
        self.size
    }

    /// Record the geometry produced by a layout pass.
    pub fn set_layout(&mut self, origin: Point, size: Size) {
        self.origin = origin;
        self.size = size;
    }

    /// The pending pass flags for this pod.
    pub fn flags(&self) -> ChangeFlags {
        self.flags
    }

    /// Merge additional pending flags (e.g. from a rebuild).
    pub fn merge_flags(&mut self, flags: ChangeFlags) {
        self.flags |= flags;
    }

    /// Clear all pending flags (after the corresponding passes have run).
    pub fn clear_flags(&mut self) {
        self.flags = ChangeFlags::NONE;
    }
}

/// One node of a read-only widget-tree snapshot, as produced by
/// [`WidgetTree::inspect`].
///
/// Plain owned data with no borrow of the tree, so a caller (a devtools service,
/// a test) can hold or forward it freely. Deliberately renderer-agnostic and
/// serialization-agnostic: geometry is `kurbo`, and any wire format belongs to
/// the consumer, not to this crate.
#[derive(Debug, Clone, PartialEq)]
pub struct InspectNode {
    /// The node's stable [`WidgetId`].
    pub id: WidgetId,
    /// The parent's id, or `None` for a root.
    pub parent: Option<WidgetId>,
    /// The concrete widget type's name — see [`WidgetPod::type_name`].
    pub type_name: &'static str,
    /// The pod's optional debug label — see [`WidgetPod::set_debug_label`].
    pub debug_label: Option<Cow<'static, str>>,
    /// The widget's border box in **absolute** window coordinates, logical px:
    /// the pod's recorded layout origin accumulated down from the root, plus its
    /// recorded size. `Rect::ZERO`-sized until a layout pass has run.
    pub bounds: Rect,
    /// This node's children: the arena's own children first (insertion order),
    /// then the widget's owned [`ChildPod`]s in declaration order.
    pub children: Vec<WidgetId>,
    /// Distance from the root (roots are `0`).
    pub depth: usize,
}

/// The widget tree: a thin, typed wrapper over `tree_arena::TreeArena`.
pub struct WidgetTree {
    arena: TreeArena<WidgetPod>,
    /// Roots in insertion order. Ordering only — the arena owns membership.
    root_order: Vec<WidgetId>,
    /// Per-parent child ids in insertion order. Same rule: ordering only.
    child_order: HashMap<WidgetId, Vec<WidgetId>>,
}

impl WidgetTree {
    /// Create an empty tree.
    pub fn new() -> Self {
        Self {
            arena: TreeArena::new(),
            root_order: Vec::new(),
            child_order: HashMap::new(),
        }
    }

    /// Insert `pod` as a root of the tree, returning its id.
    pub fn insert_root(&mut self, pod: WidgetPod) -> WidgetId {
        let id = pod.id();
        // `ArenaMutList::insert` takes `impl Into<NodeId>`; `WidgetId -> u64`.
        self.arena.roots_mut().insert(id.0, pod);
        self.root_order.push(id);
        id
    }

    /// Insert `pod` as a child of `parent`. Returns the child id, or `None` if
    /// `parent` is not in the tree.
    ///
    /// Exercises the tree_arena 0.2.0 `ArenaMut` / `ArenaMutList` handles: a
    /// node's value and its child list are borrowed disjointly, so this stays
    /// borrow-checker-clean.
    pub fn insert_child(&mut self, parent: WidgetId, pod: WidgetPod) -> Option<WidgetId> {
        let id = pod.id();
        let mut parent_mut: ArenaMut<'_, WidgetPod> = self.arena.find_mut(parent.0)?;
        parent_mut.children.insert(id.0, pod);
        self.child_order.entry(parent).or_default().push(id);
        Some(id)
    }

    /// Shared access to the pod with the given id, anywhere in the tree.
    pub fn pod(&self, id: WidgetId) -> Option<&WidgetPod> {
        self.arena.find(id.0).map(|node| node.item)
    }

    /// Mutable access to the pod with the given id, anywhere in the tree.
    pub fn pod_mut(&mut self, id: WidgetId) -> Option<&mut WidgetPod> {
        self.arena.find_mut(id.0).map(|node| node.item)
    }

    /// The tree's root ids, in insertion order.
    ///
    /// O(roots). Read-only; nothing here mutates the arena.
    pub fn roots(&self) -> Vec<WidgetId> {
        let live: HashSet<u64> = self.arena.root_ids().collect();
        order_against(&self.root_order, &live)
    }

    /// The children of `id`, in insertion order — empty if `id` has no children
    /// or is not in the tree.
    ///
    /// O(depth) to find the parent, then O(children). Every id returned is live
    /// in the arena at call time.
    pub fn children(&self, id: WidgetId) -> Vec<WidgetId> {
        match self.arena.find(id.0) {
            Some(node) => self.ordered_children(id, node),
            None => Vec::new(),
        }
    }

    /// A read-only snapshot of the whole retained tree in pre-order (a node
    /// precedes its descendants; siblings follow declaration order).
    ///
    /// The walk covers **both** child mechanisms: the arena's own children, then
    /// the [`ChildPod`]s a widget publishes through [`Widget::visit_children`] —
    /// which is where nearly the whole hierarchy actually lives. A container
    /// that does not override that seam reads as a leaf.
    ///
    /// O(nodes) — one pass, one `HashSet` of sibling ids per arena parent, no
    /// arena mutation and no bookkeeping left behind (a pod's tooling id is
    /// assigned once, on its first visit, and reused). Bounds come from what the
    /// last layout pass recorded on each pod ([`WidgetPod::origin`] /
    /// [`WidgetPod::size`], [`ChildPod::origin`] / [`ChildPod::size`]),
    /// accumulated into absolute window coordinates on the way down; call it
    /// after a layout pass or the rects are all zero-sized.
    pub fn inspect(&self) -> Vec<InspectNode> {
        let mut out = Vec::new();
        let roots = self.arena.roots();
        for id in self.roots() {
            if let Some(node) = roots.into_item(id.0) {
                self.inspect_node(node, id, None, Point::ZERO, 0, &mut out);
            }
        }
        out
    }

    /// Push `node`'s snapshot, then recurse into its arena children and its
    /// widget's owned pods. `parent_origin` is the parent's absolute origin; a
    /// pod's own origin is relative to it.
    fn inspect_node(
        &self,
        node: ArenaRef<'_, WidgetPod>,
        id: WidgetId,
        parent: Option<WidgetId>,
        parent_origin: Point,
        depth: usize,
        out: &mut Vec<InspectNode>,
    ) {
        let pod = node.item;
        let origin = parent_origin + pod.origin().to_vec2();
        let arena_children = self.ordered_children(id, node);
        // Reserve this node's slot before descending, so pre-order holds and the
        // `children` list can be filled in from the descent itself.
        let slot = out.len();
        out.push(InspectNode {
            id,
            parent,
            type_name: pod.type_name(),
            debug_label: pod.debug_label.clone(),
            bounds: Rect::from_origin_size(origin, pod.size()),
            children: Vec::new(),
            depth,
        });
        let mut children = Vec::with_capacity(arena_children.len());
        for child_id in arena_children {
            if let Some(child) = node.children.into_item(child_id.0) {
                children.push(child_id);
                self.inspect_node(child, child_id, Some(id), origin, depth + 1, out);
            }
        }
        Self::inspect_pods(pod.widget(), id, origin, depth, &mut children, out);
        out[slot].children = children;
    }

    /// Push a snapshot for each of `widget`'s owned [`ChildPod`]s (and, through
    /// them, the whole subtree below), recording their ids into `children`.
    fn inspect_pods(
        widget: &dyn Widget,
        parent: WidgetId,
        parent_origin: Point,
        parent_depth: usize,
        children: &mut Vec<WidgetId>,
        out: &mut Vec<InspectNode>,
    ) {
        widget.visit_children(&mut |child| {
            let child_id = child.inspect_id();
            children.push(child_id);
            Self::inspect_child(
                child,
                child_id,
                parent,
                parent_origin,
                parent_depth + 1,
                out,
            );
        });
    }

    /// The [`ChildPod`] mirror of [`WidgetTree::inspect_node`]: emit `child`'s
    /// own node, then descend into whatever it owns.
    fn inspect_child(
        child: &ChildPod,
        id: WidgetId,
        parent: WidgetId,
        parent_origin: Point,
        depth: usize,
        out: &mut Vec<InspectNode>,
    ) {
        let origin = parent_origin + child.origin().to_vec2();
        let slot = out.len();
        out.push(InspectNode {
            id,
            parent: Some(parent),
            type_name: child.type_name(),
            debug_label: child.debug_label_cow(),
            bounds: Rect::from_origin_size(origin, child.size()),
            children: Vec::new(),
            depth,
        });
        let mut grandchildren = Vec::new();
        Self::inspect_pods(child.widget(), id, origin, depth, &mut grandchildren, out);
        out[slot].children = grandchildren;
    }

    /// The arena's children of `node`, ordered by the recorded insertion index.
    fn ordered_children(&self, id: WidgetId, node: ArenaRef<'_, WidgetPod>) -> Vec<WidgetId> {
        let live: HashSet<u64> = node.child_ids().into_iter().collect();
        let recorded = self.child_order.get(&id).map_or(&[][..], Vec::as_slice);
        order_against(recorded, &live)
    }
}

/// Order `live` (the arena's authoritative id set) by `recorded` (the insertion
/// index).
///
/// The arena wins on membership: a recorded id the arena no longer holds is
/// dropped, and a live id the index never saw is appended in id order rather
/// than lost. That keeps traversal truthful even if a future mutation path
/// forgets to update the index.
fn order_against(recorded: &[WidgetId], live: &HashSet<u64>) -> Vec<WidgetId> {
    let mut ordered: Vec<WidgetId> = recorded
        .iter()
        .copied()
        .filter(|id| live.contains(&id.0))
        .collect();
    if ordered.len() != live.len() {
        let seen: HashSet<u64> = ordered.iter().map(|id| id.0).collect();
        let mut unrecorded: Vec<u64> = live.difference(&seen).copied().collect();
        unrecorded.sort_unstable();
        ordered.extend(unrecorded.into_iter().map(WidgetId));
    }
    ordered
}

impl Default for WidgetTree {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::BoxConstraints;
    use crate::widget::{LayoutCtx, PaintCtx, PaintScene};

    struct Leaf;
    impl Widget for Leaf {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
    }

    /// A container in the shape every real one has: children owned as
    /// [`ChildPod`]s, published through [`Widget::visit_children`].
    struct Container {
        children: Vec<ChildPod>,
    }

    impl Container {
        /// A container over `children`, each placed at `origin` with `size` the
        /// way a layout pass would have left it.
        fn new(children: Vec<(Point, Size, Box<dyn Widget>)>) -> Self {
            Self {
                children: children
                    .into_iter()
                    .map(|(origin, size, widget)| {
                        let mut pod = ChildPod::new(widget);
                        pod.set_origin(origin);
                        // A pod records its size from `layout_child`; these tests
                        // assert the walk, not layout, so drive it directly.
                        pod.layout_child(&mut LayoutCtx::new(), &BoxConstraints::new(size, size));
                        pod
                    })
                    .collect(),
            }
        }
    }

    impl Widget for Container {
        fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
            bc.max()
        }
        fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        fn visit_children(&self, visitor: &mut dyn FnMut(&ChildPod)) {
            for child in &self.children {
                visitor(child);
            }
        }
    }

    fn pod(id: u64) -> WidgetPod {
        WidgetPod::new(WidgetId(id), Box::new(Leaf))
    }

    #[test]
    fn insert_root_and_find() {
        let mut tree = WidgetTree::new();
        let id = tree.insert_root(pod(1));
        assert_eq!(id, WidgetId(1));
        assert!(tree.pod(id).is_some());
        assert!(tree.pod_mut(id).is_some());
        assert!(tree.pod(WidgetId(999)).is_none());
    }

    #[test]
    fn insert_child_traverses_arena() {
        let mut tree = WidgetTree::new();
        let root = tree.insert_root(pod(1));
        let child = tree.insert_child(root, pod(2)).expect("child inserted");
        assert_eq!(child, WidgetId(2));
        // The child is reachable via the arena's global find (descendant lookup).
        assert!(tree.pod(child).is_some());
        // Inserting under a missing parent fails cleanly.
        assert!(tree.insert_child(WidgetId(42), pod(3)).is_none());
    }

    #[test]
    fn roots_and_children_follow_insertion_order() {
        let mut tree = WidgetTree::new();
        // Two roots, inserted high id first: recorded order, not id order.
        let root_b = tree.insert_root(pod(20));
        let root_a = tree.insert_root(pod(10));
        assert_eq!(tree.roots(), vec![root_b, root_a]);

        let c2 = tree.insert_child(root_b, pod(22)).unwrap();
        let c1 = tree.insert_child(root_b, pod(21)).unwrap();
        let grandchild = tree.insert_child(c2, pod(30)).unwrap();
        assert_eq!(tree.children(root_b), vec![c2, c1]);
        assert_eq!(tree.children(c2), vec![grandchild]);
        // Leaves and unknown ids both report no children, never a dangling id.
        assert!(tree.children(c1).is_empty());
        assert!(tree.children(WidgetId(999)).is_empty());
    }

    #[test]
    fn pod_records_type_name_and_optional_label() {
        let mut tree = WidgetTree::new();
        // The erased constructor cannot name the concrete type.
        let erased = tree.insert_root(WidgetPod::new(WidgetId(1), Box::new(Leaf)));
        assert_eq!(
            tree.pod(erased).unwrap().type_name(),
            WidgetPod::ERASED_TYPE_NAME
        );

        let typed = tree
            .insert_child(erased, WidgetPod::new_typed(WidgetId(2), Leaf))
            .unwrap();
        let pod = tree.pod_mut(typed).unwrap();
        assert!(pod.type_name().ends_with("Leaf"), "{}", pod.type_name());
        // Labels default to absent, are set on demand, and clear again.
        assert_eq!(pod.debug_label(), None);
        pod.set_debug_label("the-leaf");
        assert_eq!(pod.debug_label(), Some("the-leaf"));
        pod.clear_debug_label();
        assert_eq!(pod.debug_label(), None);
    }

    #[test]
    fn inspect_walks_pre_order_with_absolute_bounds() {
        let mut tree = WidgetTree::new();
        let root = tree.insert_root(WidgetPod::new_typed(WidgetId(1), Leaf));
        let a = tree
            .insert_child(root, WidgetPod::new_typed(WidgetId(2), Leaf))
            .unwrap();
        let a_child = tree
            .insert_child(a, WidgetPod::new_typed(WidgetId(3), Leaf))
            .unwrap();
        let b = tree
            .insert_child(root, WidgetPod::new_typed(WidgetId(4), Leaf))
            .unwrap();

        // Geometry as a layout pass would record it: origins parent-relative.
        tree.pod_mut(root)
            .unwrap()
            .set_layout(Point::ZERO, Size::new(100.0, 100.0));
        tree.pod_mut(a)
            .unwrap()
            .set_layout(Point::new(10.0, 5.0), Size::new(50.0, 40.0));
        tree.pod_mut(a_child)
            .unwrap()
            .set_layout(Point::new(2.0, 3.0), Size::new(10.0, 10.0));
        tree.pod_mut(b)
            .unwrap()
            .set_layout(Point::new(0.0, 60.0), Size::new(20.0, 20.0));
        tree.pod_mut(a).unwrap().set_debug_label("branch-a");

        let nodes = tree.inspect();
        // Pre-order: root, a, a's child, b.
        let ids: Vec<WidgetId> = nodes.iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![root, a, a_child, b]);
        assert_eq!(
            nodes.iter().map(|n| n.depth).collect::<Vec<_>>(),
            vec![0, 1, 2, 1]
        );
        assert_eq!(
            nodes.iter().map(|n| n.parent).collect::<Vec<_>>(),
            vec![None, Some(root), Some(a), Some(root)]
        );
        assert_eq!(nodes[0].children, vec![a, b]);
        assert_eq!(nodes[2].children, Vec::new());

        // Bounds accumulate down: a's child sits at 10+2, 5+3.
        assert_eq!(nodes[1].bounds, Rect::new(10.0, 5.0, 60.0, 45.0));
        assert_eq!(nodes[2].bounds, Rect::new(12.0, 8.0, 22.0, 18.0));

        // Type names captured, labels default to None.
        assert!(nodes[0].type_name.ends_with("Leaf"));
        assert_eq!(nodes[1].debug_label.as_deref(), Some("branch-a"));
        assert!(nodes[0].debug_label.is_none());
        assert!(nodes[3].debug_label.is_none());
    }

    #[test]
    fn inspect_of_an_unlaid_out_tree_reports_zero_rects() {
        let mut tree = WidgetTree::new();
        let root = tree.insert_root(WidgetPod::new_typed(WidgetId(1), Leaf));
        tree.insert_child(root, WidgetPod::new_typed(WidgetId(2), Leaf))
            .unwrap();
        let nodes = tree.inspect();
        assert_eq!(nodes.len(), 2);
        assert!(nodes.iter().all(|n| n.bounds == Rect::ZERO));
    }

    #[test]
    fn traversal_follows_the_arena_not_the_order_index() {
        // The order index is ordering only: the arena decides membership. Both
        // divergence directions are covered, since `WidgetTree` exposes no
        // removal today and a future one must not be able to leak a stale id.
        let mut tree = WidgetTree::new();
        let root = tree.insert_root(pod(1));
        let kept = tree.insert_child(root, pod(2)).unwrap();

        // An id the index records but the arena never held is dropped.
        tree.child_order
            .get_mut(&root)
            .unwrap()
            .push(WidgetId(1234));
        assert_eq!(tree.children(root), vec![kept]);
        assert_eq!(tree.inspect().len(), 2);

        // An arena child the index never saw is still reported (id order).
        tree.child_order.remove(&root);
        assert_eq!(tree.children(root), vec![kept]);
        tree.root_order.clear();
        assert_eq!(tree.roots(), vec![root]);
    }

    #[test]
    fn inspect_descends_through_child_pods() {
        // The shape the arena alone cannot see: root -> container -> (leaf,
        // nested container -> leaf). Only the root is an arena node; everything
        // below it is a `ChildPod` reached through `visit_children`.
        let inner = Container::new(vec![(
            Point::new(1.0, 1.0),
            Size::new(5.0, 5.0),
            Box::new(Leaf),
        )]);
        let outer = Container::new(vec![
            (Point::new(10.0, 5.0), Size::new(50.0, 40.0), Box::new(Leaf)),
            (
                Point::new(0.0, 60.0),
                Size::new(20.0, 20.0),
                Box::new(inner) as Box<dyn Widget>,
            ),
        ]);

        let mut tree = WidgetTree::new();
        let root = tree.insert_root(WidgetPod::new_typed(WidgetId(1), outer));
        tree.pod_mut(root)
            .unwrap()
            .set_layout(Point::new(2.0, 3.0), Size::new(100.0, 100.0));

        let nodes = tree.inspect();
        assert_eq!(nodes.len(), 4, "root + two pods + one grandchild pod");

        // Pre-order: root, first pod, second pod, the nested pod under it.
        assert_eq!(
            nodes.iter().map(|n| n.depth).collect::<Vec<_>>(),
            vec![0, 1, 1, 2],
            "a pod's depth continues from its owning node's"
        );
        assert_eq!(nodes[0].parent, None);
        assert_eq!(nodes[1].parent, Some(nodes[0].id));
        assert_eq!(nodes[2].parent, Some(nodes[0].id));
        assert_eq!(nodes[3].parent, Some(nodes[2].id));
        assert_eq!(nodes[0].children, vec![nodes[1].id, nodes[2].id]);
        assert_eq!(nodes[2].children, vec![nodes[3].id]);
        assert!(nodes[1].children.is_empty(), "a leaf publishes no children");

        // Bounds accumulate through ChildPod origins, on top of the arena pod's
        // own absolute origin: root at (2,3); first pod at (2+10, 3+5); the
        // nested container at (2+0, 3+60); its child at (2+0+1, 3+60+1).
        assert_eq!(nodes[0].bounds, Rect::new(2.0, 3.0, 102.0, 103.0));
        assert_eq!(nodes[1].bounds, Rect::new(12.0, 8.0, 62.0, 48.0));
        assert_eq!(nodes[2].bounds, Rect::new(2.0, 63.0, 22.0, 83.0));
        assert_eq!(nodes[3].bounds, Rect::new(3.0, 64.0, 8.0, 69.0));

        // Type names: the arena pod's is captured by `new_typed`, a child pod's
        // by its construction path.
        assert!(
            nodes[0].type_name.ends_with("Container"),
            "{}",
            nodes[0].type_name
        );
        assert!(
            nodes[1..]
                .iter()
                .all(|n| n.type_name.ends_with("Leaf") || n.type_name.ends_with("Container"))
        );

        // Ids are unique, and every pod id is drawn from the reserved range so
        // it can never be mistaken for an arena `WidgetId`.
        let ids: HashSet<u64> = nodes.iter().map(|n| n.id.0).collect();
        assert_eq!(ids.len(), nodes.len(), "ids are unique across a snapshot");
        assert!(
            nodes[1..]
                .iter()
                .all(|n| n.id.0 >= crate::widget::ChildPod::INSPECT_ID_BASE)
        );
    }

    #[test]
    fn a_pods_tooling_id_and_label_survive_the_next_walk() {
        // Selection stability: a devtools client holding an id must still be
        // holding the same pod on the next frame's snapshot.
        let mut tree = WidgetTree::new();
        let root = tree.insert_root(WidgetPod::new_typed(
            WidgetId(1),
            Container::new(vec![(Point::ZERO, Size::new(4.0, 4.0), Box::new(Leaf))]),
        ));
        let first = tree.inspect();
        let second = tree.inspect();
        assert_eq!(first[1].id, second[1].id);

        // A label attached to a child pod reaches the snapshot, and clearing it
        // takes it back out.
        let pod = tree.pod_mut(root).unwrap();
        let container = pod
            .widget_mut()
            .downcast_mut::<Container>()
            .expect("the root widget is the container");
        container.children[0].set_debug_label("the-child");
        assert_eq!(tree.inspect()[1].debug_label.as_deref(), Some("the-child"));
        let container = tree
            .pod_mut(root)
            .unwrap()
            .widget_mut()
            .downcast_mut::<Container>()
            .expect("the root widget is the container");
        container.children[0].clear_debug_label();
        assert_eq!(tree.inspect()[1].debug_label, None);
    }

    #[test]
    fn a_widget_that_ignores_the_seam_reads_as_a_leaf() {
        // The default impl is free to ignore: a container that never overrides
        // `visit_children` contributes exactly one node, no matter what it owns.
        struct Opaque {
            _child: ChildPod,
        }
        impl Widget for Opaque {
            fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
                bc.max()
            }
            fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}
        }

        let mut tree = WidgetTree::new();
        tree.insert_root(WidgetPod::new_typed(
            WidgetId(1),
            Opaque {
                _child: ChildPod::new(Box::new(Leaf)),
            },
        ));
        assert_eq!(tree.inspect().len(), 1);
    }

    #[test]
    fn pod_stores_layout_geometry() {
        let mut tree = WidgetTree::new();
        let id = tree.insert_root(pod(1));
        let p = tree.pod_mut(id).unwrap();
        assert!(p.flags().needs_layout());
        p.set_layout(Point::new(2.0, 3.0), Size::new(10.0, 20.0));
        p.clear_flags();
        assert_eq!(p.origin(), Point::new(2.0, 3.0));
        assert_eq!(p.size(), Size::new(10.0, 20.0));
        assert!(p.flags().is_empty());
    }
}
