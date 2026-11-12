//! Semantics-only views: [`semantics(child)`](semantics) annotates a child's
//! accessibility subtree without changing how it lays out or paints, and
//! [`semantics_label(text)`](semantics_label) announces a piece of text with no
//! visible widget.
//!
//! # The wrapper
//!
//! [`SemanticsView`] lays out, paints and routes events exactly as its child
//! does. Its only contribution is to the accessibility tree: the child's
//! nodes are published beneath one container node, optionally carrying a
//! [`label`](SemanticsView::label), [`value`](SemanticsView::value),
//! [`hint`](SemanticsView::hint), a [`role`](SemanticsView::role) or a
//! [`heading`](SemanticsView::heading) level.
//!
//! # Default role
//!
//! With no explicit role the node is [`Role::Group`] as soon as a label,
//! value or hint is set, and [`Role::GenericContainer`] only for a wrapper
//! that sets none of them. Phone accessibility adapters filter
//! `GenericContainer` nodes out of the tree, so a labelled wrapper left as a
//! generic container would never become an accessibility element.
//!
//! # Merging
//!
//! [`merged(true)`](SemanticsView::merged) publishes the wrapper as ONE leaf
//! node carrying its role, label, value, hint, level and actions, and does not
//! descend into the child's semantics. The child's own nodes, including any
//! control inside, are then no longer separately reachable.
//!
//! # Hiding
//!
//! [`hidden(true)`](SemanticsView::hidden) does **not** mark nodes hidden. The
//! wrapper simply does not descend into the child's semantics, so neither the
//! container node nor any node of the subtree is present in the published
//! tree. The child still lays out, paints and receives events.

use frust_core::{
    BoxConstraints, BuildCtx, ChangeFlags, ChildPod, EventCtx, EventResult, InputEvent, LayoutCtx,
    PaintCtx, PaintScene, SemanticsCtx, View, Widget, any,
};
use kurbo::{Point, Size};

use frust_core::accesskit::{Action, Node, Role};

/// The values a [`SemanticsView`] publishes, shared by the view and its widget.
#[derive(Clone, PartialEq)]
struct Published {
    label: Option<String>,
    value: Option<String>,
    hint: Option<String>,
    /// `None` until set; resolved to a default at publish time.
    role: Option<Role>,
    level: Option<usize>,
    hidden: bool,
    merged: bool,
    tappable: bool,
}

impl Published {
    /// The explicit role, else [`Role::Group`] when any descriptive text is
    /// set, else [`Role::GenericContainer`] (a pure pass-through).
    fn resolved_role(&self) -> Role {
        match self.role {
            Some(role) => role,
            None if self.label.is_some() || self.value.is_some() || self.hint.is_some() => {
                Role::Group
            }
            None => Role::GenericContainer,
        }
    }

    fn fill(&self, node: &mut Node) {
        if let Some(label) = &self.label {
            node.set_label(label.as_str());
        }
        if let Some(value) = &self.value {
            node.set_value(value.as_str());
        }
        if let Some(hint) = &self.hint {
            node.set_description(hint.as_str());
        }
        if let Some(level) = self.level {
            node.set_level(level);
        }
        if self.tappable {
            node.add_action(Action::Click);
        }
    }
}

/// A declarative semantics wrapper. See the [module docs](self).
pub struct SemanticsView<State: 'static> {
    child: frust_core::AnyView<State>,
    props: Published,
}

/// Wrap `child` so its accessibility subtree sits under one container node
/// that this view can label, re-role, merge or omit. Layout and paint are the
/// child's own.
pub fn semantics<State: 'static, V: View<State>>(child: V) -> SemanticsView<State> {
    SemanticsView {
        child: any(child),
        props: Published {
            label: None,
            value: None,
            hint: None,
            role: None,
            level: None,
            hidden: false,
            merged: false,
            tappable: false,
        },
    }
}

impl<State: 'static> SemanticsView<State> {
    /// Set the accessible label of the node.
    pub fn label(mut self, text: impl Into<String>) -> Self {
        self.props.label = Some(text.into());
        self
    }

    /// Set the accessible value of the node.
    pub fn value(mut self, text: impl Into<String>) -> Self {
        self.props.value = Some(text.into());
        self
    }

    /// Set the accessible hint of the node (published as the description,
    /// which iOS reads as the hint).
    pub fn hint(mut self, text: impl Into<String>) -> Self {
        self.props.hint = Some(text.into());
        self
    }

    /// Set the role of the node. Unset, the role is [`Role::Group`] when a
    /// label, value or hint is set and [`Role::GenericContainer`] otherwise.
    /// A role other than [`Role::Heading`] drops any level set earlier by
    /// [`heading`](Self::heading), so the last call wins.
    pub fn role(mut self, role: Role) -> Self {
        if role != Role::Heading {
            self.props.level = None;
        }
        self.props.role = Some(role);
        self
    }

    /// Mark the node as a heading of `level` (1 is the top level, as in
    /// HTML). Sets the role to [`Role::Heading`]; the platform level is
    /// zero-based, so `level - 1` is stored. A `level` of 0 is clamped to 1,
    /// the same node as `heading(1)`.
    ///
    /// Also [`merged`](Self::merged): a heading container whose child is
    /// separately exposed is dropped on iOS, so the heading is published as
    /// a leaf.
    pub fn heading(mut self, level: usize) -> Self {
        self.props.role = Some(Role::Heading);
        self.props.level = Some(level.saturating_sub(1));
        self.props.merged = true;
        self
    }

    /// Publish ONE leaf node carrying this wrapper's role, label, value,
    /// hint, level and actions, without descending into the child's
    /// semantics. The child's own nodes, including any control inside, are no
    /// longer separately reachable.
    pub fn merged(mut self, merged: bool) -> Self {
        self.props.merged = merged;
        self
    }

    /// Add [`Action::Click`] to the node. Accessibility actions are
    /// positional: the framework delivers a click as a tap at the centre of
    /// the node's bounds, so the tappable child (a gesture detector or a
    /// button) must cover the wrapper's centre.
    pub fn tappable(mut self, tappable: bool) -> Self {
        self.props.tappable = tappable;
        self
    }

    /// An image: [`Role::Image`], [`merged`](Self::merged).
    pub fn image(mut self) -> Self {
        self.props.role = Some(Role::Image);
        self.props.level = None;
        self.props.merged = true;
        self
    }

    /// A button: [`Role::Button`], [`merged`](Self::merged) and
    /// [`tappable`](Self::tappable). The child's own nodes are not separately
    /// reachable; see [`tappable`](Self::tappable) for how the tap lands.
    pub fn button(mut self) -> Self {
        self.props.role = Some(Role::Button);
        self.props.level = None;
        self.props.merged = true;
        self.props.tappable = true;
        self
    }

    /// Omit the whole subtree, container included, from the accessibility
    /// tree. The subtree is omitted, not marked hidden: no node of it is
    /// published. The child still lays out, paints and handles events.
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.props.hidden = hidden;
        self
    }
}

/// The retained widget for a [`SemanticsView`].
pub struct SemanticsWidget {
    child: ChildPod,
    props: Published,
}

impl<State: 'static> View<State> for SemanticsView<State> {
    type Element = SemanticsWidget;

    fn build(&self, ctx: &mut BuildCtx<'_>) -> SemanticsWidget {
        SemanticsWidget {
            child: crate::authoring::build_child(&self.child, ctx),
            props: self.props.clone(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SemanticsWidget,
        ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        // The published node is read from this widget when the tree is
        // published, and the semantics generation advances only on a non-empty
        // rebuild. A change to any published value therefore raises PAINT, as
        // `IconButtonWidget::rebuild` does for its label.
        let published_changed = prev.props != self.props;
        if published_changed {
            element.props = self.props.clone();
        }
        let mut flags =
            crate::authoring::rebuild_child(&prev.child, &self.child, &mut element.child, ctx);
        if published_changed {
            flags |= ChangeFlags::PAINT;
        }
        flags
    }

    fn teardown(&self, element: &mut SemanticsWidget, ctx: &mut BuildCtx<'_>) {
        crate::authoring::teardown_child(&self.child, &mut element.child, ctx);
    }
}

impl Widget for SemanticsWidget {
    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        let size = self.child.layout_child(ctx, bc);
        self.child.set_origin(Point::ZERO);
        size
    }

    fn paint(&mut self, ctx: &mut PaintCtx, scene: &mut dyn PaintScene) {
        self.child.paint_child(ctx, scene);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &InputEvent) -> EventResult {
        crate::authoring::route_event_single(&mut self.child, ctx, event)
    }

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        if self.props.hidden {
            return;
        }
        let role = self.props.resolved_role();
        if self.props.merged {
            ctx.push_node(role, |node| self.props.fill(node));
            return;
        }
        let child = &self.child;
        ctx.push_container(
            role,
            |node| self.props.fill(node),
            |ctx| child.semantics_child(ctx),
        );
    }

    // Draws nothing of its own.
    fn hit_test_self(&self) -> bool {
        false
    }

    crate::authoring::visit_children!(child);
}

/// A declarative zero-size leaf that announces `text`. See [`semantics_label`].
pub struct SemanticsLabelView {
    text: String,
}

/// A zero-size leaf with no visible output that publishes one
/// [`Role::Label`] node carrying `text` (as its value, which is what
/// platforms read for a static-text node). Use it to announce a screen title
/// without a visible widget.
pub fn semantics_label(text: impl Into<String>) -> SemanticsLabelView {
    SemanticsLabelView { text: text.into() }
}

/// The retained widget for a [`SemanticsLabelView`].
pub struct SemanticsLabelWidget {
    text: String,
}

impl<State: 'static> View<State> for SemanticsLabelView {
    type Element = SemanticsLabelWidget;

    fn build(&self, _ctx: &mut BuildCtx<'_>) -> SemanticsLabelWidget {
        SemanticsLabelWidget {
            text: self.text.clone(),
        }
    }

    fn rebuild(
        &self,
        prev: &Self,
        element: &mut SemanticsLabelWidget,
        _ctx: &mut BuildCtx<'_>,
    ) -> ChangeFlags {
        if prev.text != self.text {
            element.text = self.text.clone();
            // The announced text is published from this widget, so a change
            // raises PAINT to advance the semantics generation.
            return ChangeFlags::PAINT;
        }
        ChangeFlags::NONE
    }
}

impl Widget for SemanticsLabelWidget {
    fn layout(&mut self, _ctx: &mut LayoutCtx, bc: &BoxConstraints) -> Size {
        bc.constrain(Size::ZERO)
    }

    fn paint(&mut self, _ctx: &mut PaintCtx, _scene: &mut dyn PaintScene) {}

    fn semantics(&self, ctx: &mut SemanticsCtx) {
        ctx.push_node(Role::Label, |node| {
            node.set_value(self.text.as_str());
        });
    }

    fn hit_test_self(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{RecordingScene, leaf};
    use frust_core::RenderRoot;
    use frust_core::accesskit::Action;
    use std::any::Any;

    fn semantics_of<V: View<()>>(
        mut logic: impl FnMut(&mut ()) -> V,
    ) -> frust_core::SemanticsUpdate {
        let mut root: RenderRoot<(), V> = RenderRoot::new();
        root.rebuild(&mut logic, &mut ());
        let mut tcx = frust_text::TextContext::new();
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        root.semantics()
    }

    fn count(update: &frust_core::SemanticsUpdate, role: Role) -> usize {
        update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == role)
            .count()
    }

    #[test]
    fn publishes_label_and_role_and_keeps_child_nodes_beneath() {
        let update = semantics_of(|_| {
            semantics(crate::button("go", |_: &mut ()| {}))
                .label("group")
                .role(Role::Group)
        });
        let (gid, group) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("wrapper publishes its container");
        assert_eq!(group.label(), Some("group"));
        let (bid, _) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("child's node is kept");
        let (parent_id, _) = update
            .nodes
            .iter()
            .find(|(_, n)| n.children().contains(bid))
            .expect("child's node has a parent");
        assert_eq!(parent_id, gid, "the container is the child's parent");
    }

    #[test]
    fn heading_sets_role_and_zero_based_level() {
        let update = semantics_of(|_| semantics(leaf(10.0, 10.0)).heading(2));
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Heading)
            .expect("heading node");
        assert_eq!(node.level(), Some(1));
    }

    #[test]
    fn hidden_leaves_no_node_from_the_subtree() {
        let update = semantics_of(|_| {
            semantics(crate::button("go", |_: &mut ()| {}))
                .label("group")
                .hidden(true)
        });
        assert_eq!(count(&update, Role::Button), 0);
        assert_eq!(count(&update, Role::GenericContainer), 0);
        assert!(update.nodes.iter().all(|(_, n)| n.label() != Some("group")));
    }

    #[test]
    fn semantics_label_is_zero_size_with_one_node() {
        let update = semantics_of(|_| semantics_label("Settings"));
        let labels: Vec<_> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Label)
            .collect();
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0].1.value(), Some("Settings"));

        let mut w = <SemanticsLabelView as View<()>>::build(
            &semantics_label("x"),
            &mut BuildCtx::new(&mut 0u64),
        );
        let size = w.layout(
            &mut LayoutCtx::new(),
            &BoxConstraints::loose(Size::new(200.0, 200.0)),
        );
        assert_eq!(size, Size::ZERO);
    }

    #[test]
    fn layout_and_paint_match_the_bare_child() {
        let bc = BoxConstraints::loose(Size::new(200.0, 200.0));

        let mut bare = <crate::test_support::Leaf as View<()>>::build(
            &leaf(30.0, 12.0),
            &mut BuildCtx::new(&mut 0u64),
        );
        let bare_size = bare.layout(&mut LayoutCtx::new(), &bc);
        let mut bare_scene = RecordingScene::default();
        bare.paint(&mut PaintCtx::new(Point::ZERO, bare_size), &mut bare_scene);

        let view: SemanticsView<()> = semantics(leaf(30.0, 12.0)).label("x");
        let mut wrapped = view.build(&mut BuildCtx::new(&mut 0u64));
        let wrapped_size = wrapped.layout(&mut LayoutCtx::new(), &bc);
        let mut wrapped_scene = RecordingScene::default();
        wrapped.paint(
            &mut PaintCtx::new(Point::ZERO, wrapped_size),
            &mut wrapped_scene,
        );

        assert_eq!(bare_size, wrapped_size);
        assert_eq!(bare_scene.rects, wrapped_scene.rects);
        assert_eq!(bare_scene.layers, wrapped_scene.layers);
        assert_eq!(bare_scene.transforms, wrapped_scene.transforms);
    }

    #[test]
    fn role_after_heading_drops_the_level() {
        let update = semantics_of(|_| semantics(leaf(10.0, 10.0)).heading(2).role(Role::Group));
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("container keeps the later role");
        assert_eq!(node.level(), None);
    }

    #[test]
    fn heading_zero_is_clamped_to_level_one() {
        let update = semantics_of(|_| semantics(leaf(10.0, 10.0)).heading(0));
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Heading)
            .expect("heading node");
        assert_eq!(node.level(), Some(0));
    }

    /// The values a generation-gated test turns one at a time.
    #[derive(Clone)]
    struct Knobs {
        label: &'static str,
        role: Role,
        heading: Option<usize>,
        hidden: bool,
        text: &'static str,
        value: Option<&'static str>,
        hint: Option<&'static str>,
        merged: bool,
        tappable: bool,
    }

    fn base() -> Knobs {
        Knobs {
            label: "group",
            role: Role::Group,
            heading: None,
            hidden: false,
            text: "3 unread",
            value: None,
            hint: None,
            merged: false,
            tappable: false,
        }
    }

    fn wrapper(k: &Knobs) -> SemanticsView<Knobs> {
        let view = semantics(crate::button("go", |_: &mut Knobs| {}))
            .label(k.label)
            .role(k.role)
            .hidden(k.hidden);
        match k.heading {
            Some(level) => view.heading(level),
            None => view,
        }
    }

    fn announce(k: &Knobs) -> SemanticsLabelView {
        semantics_label(k.text)
    }

    /// Build `first`, lay out, then rebuild the same root with `second` and
    /// lay out again. Returns the update a shell would pull, or `None` when
    /// the generation gate says the tree is unchanged.
    fn gated_update<V: View<Knobs>>(
        view: fn(&Knobs) -> V,
        first: &Knobs,
        second: &Knobs,
    ) -> Option<frust_core::SemanticsUpdate> {
        let mut root: RenderRoot<Knobs, V> = RenderRoot::new();
        let mut tcx = frust_text::TextContext::new();
        let mut state = first.clone();
        root.rebuild(&mut |k: &mut Knobs| view(k), &mut state);
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        let seen = root.semantics_generation();
        state = second.clone();
        root.rebuild(&mut |k: &mut Knobs| view(k), &mut state);
        root.layout_with_text(Size::new(200.0, 200.0), &mut tcx as &mut dyn Any);
        root.semantics_if_changed(seen)
    }

    #[test]
    fn label_only_change_reaches_the_gated_tree() {
        let second = Knobs {
            label: "renamed",
            ..base()
        };
        let update = gated_update(wrapper, &base(), &second).expect("label change is published");
        let (_, group) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("container");
        assert_eq!(group.label(), Some("renamed"));
    }

    #[test]
    fn role_only_change_reaches_the_gated_tree() {
        let second = Knobs {
            role: Role::Navigation,
            ..base()
        };
        let update = gated_update(wrapper, &base(), &second).expect("role change is published");
        assert_eq!(count(&update, Role::Navigation), 1);
        assert_eq!(count(&update, Role::Group), 0);
    }

    #[test]
    fn heading_level_only_change_reaches_the_gated_tree() {
        let first = Knobs {
            heading: Some(1),
            ..base()
        };
        let second = Knobs {
            heading: Some(2),
            ..base()
        };
        let update = gated_update(wrapper, &first, &second).expect("level change is published");
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Heading)
            .expect("heading node");
        assert_eq!(node.level(), Some(1));
    }

    #[test]
    fn hidden_only_change_reaches_the_gated_tree() {
        let second = Knobs {
            hidden: true,
            ..base()
        };
        let update = gated_update(wrapper, &base(), &second).expect("hidden change is published");
        assert_eq!(count(&update, Role::Group), 0);
    }

    #[test]
    fn label_text_only_change_reaches_the_gated_tree() {
        let second = Knobs {
            text: "4 unread",
            ..base()
        };
        let update = gated_update(announce, &base(), &second).expect("text change is published");
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Label)
            .expect("label node");
        assert_eq!(node.value(), Some("4 unread"));
    }

    #[test]
    fn unchanged_rebuild_does_not_advance_the_gate() {
        assert!(gated_update(wrapper, &base(), &base()).is_none());
        assert!(gated_update(announce, &base(), &base()).is_none());
    }

    #[test]
    fn default_role_is_group_with_text_and_generic_without() {
        let labelled = semantics_of(|_| semantics(leaf(10.0, 10.0)).label("x"));
        assert_eq!(count(&labelled, Role::Group), 1);
        assert_eq!(count(&labelled, Role::GenericContainer), 0);
        let valued = semantics_of(|_| semantics(leaf(10.0, 10.0)).value("x"));
        assert_eq!(count(&valued, Role::Group), 1);
        let hinted = semantics_of(|_| semantics(leaf(10.0, 10.0)).hint("x"));
        assert_eq!(count(&hinted, Role::Group), 1);
        let bare = semantics_of(|_| semantics(leaf(10.0, 10.0)));
        assert_eq!(count(&bare, Role::GenericContainer), 1);
        assert_eq!(count(&bare, Role::Group), 0);
    }

    #[test]
    fn value_and_hint_are_published() {
        let update = semantics_of(|_| semantics(leaf(10.0, 10.0)).value("3").hint("opens it"));
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("group");
        assert_eq!(node.value(), Some("3"));
        assert_eq!(node.description(), Some("opens it"));
    }

    #[test]
    fn merged_publishes_one_node_and_none_of_the_child() {
        let update = semantics_of(|_| {
            semantics(crate::button("go", |_: &mut ()| {}))
                .label("tile")
                .merged(true)
        });
        assert_eq!(count(&update, Role::Button), 0);
        let (gid, group) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("merged node");
        assert_eq!(group.label(), Some("tile"));
        assert!(group.children().is_empty());
        // The window root plus exactly one wrapper node.
        assert_eq!(update.nodes.len(), 2);
        let (_, root) = update
            .nodes
            .iter()
            .find(|(id, _)| *id == update.root)
            .expect("root");
        assert!(root.children().contains(gid));
    }

    #[test]
    fn tappable_adds_click() {
        let plain = semantics_of(|_| semantics(leaf(10.0, 10.0)).label("x"));
        let (_, node) = plain
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("group");
        assert!(!node.supports_action(Action::Click));
        let tap = semantics_of(|_| semantics(leaf(10.0, 10.0)).label("x").tappable(true));
        let (_, node) = tap
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Group)
            .expect("group");
        assert!(node.supports_action(Action::Click));
    }

    #[test]
    fn image_and_button_publish_leaves_with_their_role() {
        let image = semantics_of(|_| {
            semantics(crate::button("go", |_: &mut ()| {}))
                .image()
                .label("photo")
        });
        assert_eq!(count(&image, Role::Image), 1);
        assert_eq!(count(&image, Role::Button), 0);
        let button = semantics_of(|_| {
            semantics(crate::button("go", |_: &mut ()| {}))
                .button()
                .label("Open")
        });
        let (_, node) = button
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Button)
            .expect("button");
        assert_eq!(node.label(), Some("Open"));
        assert!(node.supports_action(Action::Click));
        assert!(node.children().is_empty());
        assert_eq!(button.nodes.len(), 2);
    }

    #[test]
    fn heading_is_a_leaf() {
        let update = semantics_of(|_| {
            semantics(crate::button("go", |_: &mut ()| {}))
                .heading(3)
                .label("Title")
        });
        assert_eq!(count(&update, Role::Button), 0);
        let (_, node) = update
            .nodes
            .iter()
            .find(|(_, n)| n.role() == Role::Heading)
            .expect("heading");
        assert_eq!(node.level(), Some(2));
        assert!(node.children().is_empty());
        assert_eq!(update.nodes.len(), 2);
    }

    fn knob_wrapper(k: &Knobs) -> SemanticsView<Knobs> {
        let mut view = semantics(crate::button("go", |_: &mut Knobs| {})).label("group");
        if let Some(v) = k.value {
            view = view.value(v);
        }
        if let Some(h) = k.hint {
            view = view.hint(h);
        }
        view.merged(k.merged).tappable(k.tappable)
    }

    #[test]
    fn value_hint_merged_tappable_each_advance_the_generation() {
        let cases = [
            Knobs {
                value: Some("v"),
                ..base()
            },
            Knobs {
                hint: Some("h"),
                ..base()
            },
            Knobs {
                merged: true,
                ..base()
            },
            Knobs {
                tappable: true,
                ..base()
            },
        ];
        for second in cases {
            assert!(
                gated_update(knob_wrapper, &base(), &second).is_some(),
                "a single changed value must be published"
            );
        }
        assert!(gated_update(knob_wrapper, &base(), &base()).is_none());
    }
}
