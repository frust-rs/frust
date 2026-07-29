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

use kurbo::{Point, Size};
use tree_arena::{ArenaMut, TreeArena};

use crate::view::{ChangeFlags, WidgetId};
use crate::widget::Widget;

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
}

impl WidgetPod {
    /// Wrap a freshly built widget. It starts dirty for layout and paint.
    pub fn new(id: WidgetId, widget: Box<dyn Widget>) -> Self {
        Self {
            id,
            widget,
            origin: Point::ZERO,
            size: Size::ZERO,
            flags: ChangeFlags::LAYOUT | ChangeFlags::PAINT,
        }
    }

    /// This pod's stable identity.
    pub fn id(&self) -> WidgetId {
        self.id
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

/// The widget tree: a thin, typed wrapper over `tree_arena::TreeArena`.
pub struct WidgetTree {
    arena: TreeArena<WidgetPod>,
}

impl WidgetTree {
    /// Create an empty tree.
    pub fn new() -> Self {
        Self {
            arena: TreeArena::new(),
        }
    }

    /// Insert `pod` as a root of the tree, returning its id.
    pub fn insert_root(&mut self, pod: WidgetPod) -> WidgetId {
        let id = pod.id();
        // `ArenaMutList::insert` takes `impl Into<NodeId>`; `WidgetId -> u64`.
        self.arena.roots_mut().insert(id.0, pod);
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
