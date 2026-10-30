//! The one `SemanticsUpdate` → accesskit [`TreeUpdate`] assembly every
//! platform shell pushes to its accesskit adapter.
//!
//! # Coordinate contract
//!
//! `frust_core`'s semantics pass reports every node's `bounds` in LOGICAL
//! pixels — the widget tree's layout unit. AccessKit expects PHYSICAL pixels:
//! its `Node::transform` doc reads "AccessKit expects the final transformed
//! coordinates to be relative to the origin of the tree's container (e.g.
//! window), in physical pixels". A node's `bounds` sit in the coordinate space
//! of its nearest ancestor with a non-`None` transform, so one
//! `Affine::scale(scale)` on the ROOT node converts the whole tree; raw node
//! bounds stay logical and no other node is touched. At `scale == 1.0` the root
//! is left with no transform, per accesskit's guidance to leave `transform`
//! `None` rather than stamp an explicit identity.
//!
//! `scale` must already be sanitised — finite and strictly positive, as
//! [`sanitize_scale`](crate::sanitize_scale) guarantees for an FFI density — so
//! the stamped transform can never carry a `NaN`, infinite or zero factor.

use frust_core::SemanticsUpdate;
use frust_core::accesskit::{Affine, Node, NodeId, Tree, TreeId, TreeUpdate};

/// Assemble the accesskit [`TreeUpdate`] for one full semantics pass, with the
/// root carrying `Affine::scale(scale)` (see the module docs' coordinate
/// contract; `scale` must already be sanitised).
///
/// Every node is a "new or changed" entry (a full-tree push is valid because
/// node ids are stable across passes), `tree` names [`SemanticsUpdate::root`],
/// `tree_id` is [`TreeId::ROOT`] (a single, non-subtree tree), and `focus` is
/// [`SemanticsUpdate::focus_id`] — the focused node, else the root, since
/// accesskit's `focus` is non-optional. Clones the node list; a caller that owns
/// its update can hand the pieces to [`tree_update_from_parts`] instead.
pub fn build_tree_update(update: &SemanticsUpdate, scale: f64) -> TreeUpdate {
    tree_update_from_parts(update.nodes.clone(), update.root, update.focus_id(), scale)
}

/// [`build_tree_update`] over already-separated, owned pieces: `nodes` is moved
/// through in order, `focus` is the already-resolved focus id
/// ([`SemanticsUpdate::focus_id`]), and `scale` must already be sanitised. A
/// `nodes` list without `root` is assembled unchanged, with no transform.
pub fn tree_update_from_parts(
    mut nodes: Vec<(NodeId, Node)>,
    root: NodeId,
    focus: NodeId,
    scale: f64,
) -> TreeUpdate {
    debug_assert!(
        scale.is_finite() && scale > 0.0,
        "a11y root scale must be sanitised by the caller, got {scale}"
    );
    if scale != 1.0
        && let Some((_, root_node)) = nodes.iter_mut().find(|(id, _)| *id == root)
    {
        root_node.set_transform(Affine::scale(scale));
    }
    TreeUpdate {
        nodes,
        tree: Some(Tree::new(root)),
        tree_id: TreeId::ROOT,
        focus,
    }
}

#[cfg(test)]
mod tests {
    use super::{build_tree_update, tree_update_from_parts};
    use frust_core::SemanticsUpdate;
    use frust_core::accesskit::{Affine, Node, NodeId, Rect, Role, Tree, TreeId};

    /// A window root with one bounded button child, focused on the child.
    fn window_with_button() -> SemanticsUpdate {
        let (root, button) = (NodeId(1), NodeId(2));
        let mut root_node = Node::new(Role::Window);
        root_node.set_children(vec![button]);
        root_node.set_bounds(Rect::new(0.0, 0.0, 393.0, 851.0));
        let mut button_node = Node::new(Role::Button);
        button_node.set_bounds(Rect::new(16.0, 24.0, 120.0, 72.0));
        // Post-order, as the semantics pass collects: the root comes last.
        SemanticsUpdate {
            nodes: vec![(button, button_node), (root, root_node)],
            root,
            focus: Some(button),
        }
    }

    #[test]
    fn assembles_nodes_tree_and_focus() {
        let update = window_with_button();
        let tree_update = build_tree_update(&update, 1.0);
        assert_eq!(tree_update.nodes, update.nodes);
        assert_eq!(tree_update.tree, Some(Tree::new(NodeId(1))));
        assert_eq!(tree_update.tree_id, TreeId::ROOT);
        assert_eq!(tree_update.focus, NodeId(2));
    }

    #[test]
    fn focus_falls_back_to_root_when_unfocused() {
        let update = SemanticsUpdate {
            focus: None,
            ..window_with_button()
        };
        assert_eq!(build_tree_update(&update, 2.0).focus, NodeId(1));
    }

    #[test]
    fn scale_one_stamps_no_transform() {
        let tree_update = build_tree_update(&window_with_button(), 1.0);
        assert!(
            tree_update
                .nodes
                .iter()
                .all(|(_, node)| node.transform().is_none())
        );
    }

    #[test]
    fn root_alone_carries_the_scale_and_bounds_stay_logical() {
        let update = window_with_button();
        for scale in [2.0, 2.75] {
            let tree_update = build_tree_update(&update, scale);
            let (button_id, button) = &tree_update.nodes[0];
            let (root_id, root) = &tree_update.nodes[1];
            assert_eq!((*button_id, *root_id), (NodeId(2), NodeId(1)));
            assert_eq!(root.transform(), Some(&Affine::scale(scale)));
            assert_eq!(button.transform(), None);
            assert_eq!(button.bounds(), Some(Rect::new(16.0, 24.0, 120.0, 72.0)));
            assert_eq!(root.bounds(), Some(Rect::new(0.0, 0.0, 393.0, 851.0)));
            // Apart from the root's transform, everything is as assembled at 1x.
            assert_eq!(root.role(), Role::Window);
            assert_eq!(root.children(), &[NodeId(2)]);
            assert_eq!(button, &update.nodes[0].1);
            assert_eq!(tree_update.tree, Some(Tree::new(NodeId(1))));
            assert_eq!(tree_update.focus, NodeId(2));
        }
    }

    #[test]
    fn a_missing_root_or_an_empty_list_assembles_without_a_transform() {
        let tree_update = tree_update_from_parts(
            vec![(NodeId(2), Node::new(Role::Button))],
            NodeId(1),
            NodeId(1),
            2.75,
        );
        assert_eq!(tree_update.nodes[0].1.transform(), None);
        let empty = tree_update_from_parts(Vec::new(), NodeId(1), NodeId(1), 2.75);
        assert!(empty.nodes.is_empty());
        assert_eq!(empty.tree, Some(Tree::new(NodeId(1))));
    }
}
