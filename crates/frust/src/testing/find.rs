//! Finders: locate a node in a [`Tester`]'s semantics tree or retained widget tree.
//!
//! The semantics finders ([`text`], [`value`], [`label_text`], [`label`], [`role`]) read the
//! accessibility tree [`Tester::semantics`] returns, so they see what a screen reader sees: a
//! button is one `Role::Button` node labelled with its text, with no separate node for the text
//! inside it. The type finders ([`by_type`], [`by_type_name`]) read [`Tester::inspect`] instead, so
//! they also reach widgets that contribute no semantics node, such as a button's label.
//!
//! Matches come in tree order, parents before children, so "the first match" is the one nearest
//! the root. A static text run (`Role::Label`) carries its text in its *value* and a text field
//! carries its contents there too, while a button or a checkbox carries its text in its *label*;
//! [`text`] matches either, and [`value`] / [`label_text`] pin one of them down.

use std::collections::{HashMap, HashSet};
use std::fmt;

use frust_core::accesskit::{Node, NodeId, Role};
use frust_core::{SemanticsUpdate, View, Widget, WidgetId};
use kurbo::Rect;

use super::Tester;

/// A query over a [`Tester`]'s semantics or retained tree. Build one with the functions of this
/// module ([`text`], [`role`], [`by_type`], ..) and pass it to [`Tester::find`] and friends, or as
/// a gesture target ([`Tester::tap`] takes `impl Into<Target>`, and a `Finder` is one).
///
/// [`Target`]: super::Target
#[derive(Clone, Debug, PartialEq)]
pub struct Finder(FinderKind);

#[derive(Clone, Debug, PartialEq)]
enum FinderKind {
    /// Label or value equals the string.
    Text(String),
    /// Value equals the string.
    Value(String),
    /// Label equals the string.
    LabelText(String),
    /// Role equals the role and label equals the string.
    Label(Role, String),
    /// Role equals the role.
    Role(Role),
    /// `InspectNode::type_name` equals the string.
    Type(String),
}

/// Nodes whose label **or** value equals `s`: static text, a button's label, a text field's
/// contents. See [`value`] and [`label_text`] to match only one of the two.
pub fn text(s: &str) -> Finder {
    Finder(FinderKind::Text(s.to_owned()))
}

/// Nodes whose value equals `s`: static text and the contents of a text field or other
/// value-bearing control.
pub fn value(s: &str) -> Finder {
    Finder(FinderKind::Value(s.to_owned()))
}

/// Nodes whose label equals `s`, whatever their role: a button's or a checkbox's text.
pub fn label_text(s: &str) -> Finder {
    Finder(FinderKind::LabelText(s.to_owned()))
}

/// Nodes of role `role` whose label equals `s`, e.g. `label(Role::Button, "Save")`.
pub fn label(role: Role, s: &str) -> Finder {
    Finder(FinderKind::Label(role, s.to_owned()))
}

/// Nodes of role `role`.
pub fn role(role: Role) -> Finder {
    Finder(FinderKind::Role(role))
}

/// Retained widgets whose concrete type is `W`, matched on [`InspectNode::type_name`] against
/// `core::any::type_name::<W>()`.
///
/// This reads the retained tree, not the semantics tree, so it finds widgets that contribute no
/// accessibility node. A transparent wrapper that overrides `Widget::type_name` reports its
/// child's name instead of its own, so it cannot be found under its own type.
///
/// [`InspectNode::type_name`]: super::InspectNode::type_name
pub fn by_type<W: Widget + 'static>() -> Finder {
    Finder(FinderKind::Type(core::any::type_name::<W>().to_owned()))
}

/// Retained widgets whose [`InspectNode::type_name`] is exactly `name`, for a widget type a test
/// cannot name (a private type of another crate). Prefer [`by_type`] where the type is in reach.
///
/// [`InspectNode::type_name`]: super::InspectNode::type_name
pub fn by_type_name(name: &str) -> Finder {
    Finder(FinderKind::Type(name.to_owned()))
}

impl Finder {
    /// Whether this finder reads the semantics tree (as opposed to the retained tree).
    fn is_semantic(&self) -> bool {
        !matches!(self.0, FinderKind::Type(_))
    }

    /// The matched text when `node` matches, `None` when it does not.
    fn match_node(&self, node: &Node) -> Option<Option<String>> {
        let label = node.label();
        let value = node.value();
        let text = || label.or(value).map(str::to_owned);
        let hit = match &self.0 {
            FinderKind::Text(s) => {
                if label == Some(s.as_str()) || value == Some(s.as_str()) {
                    return Some(Some(s.clone()));
                }
                false
            }
            FinderKind::Value(s) => value == Some(s.as_str()),
            FinderKind::LabelText(s) => label == Some(s.as_str()),
            FinderKind::Label(role, s) => node.role() == *role && label == Some(s.as_str()),
            FinderKind::Role(role) => node.role() == *role,
            FinderKind::Type(_) => false,
        };
        hit.then(text)
    }
}

impl fmt::Display for Finder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            FinderKind::Text(s) => write!(f, "text {s:?}"),
            FinderKind::Value(s) => write!(f, "value {s:?}"),
            FinderKind::LabelText(s) => write!(f, "label {s:?}"),
            FinderKind::Label(role, s) => write!(f, "{role:?} labelled {s:?}"),
            FinderKind::Role(role) => write!(f, "role {role:?}"),
            FinderKind::Type(name) => write!(f, "type {name}"),
        }
    }
}

/// One node a [`Finder`] matched.
///
/// A semantics finder fills `node`, `role` and `text` (the matched label or value); a type finder
/// fills `widget` only. `bounds` is in absolute window coordinates, logical px, as of the last
/// layout.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Match {
    /// The accessibility node, for a semantics finder.
    pub node: Option<NodeId>,
    /// The retained widget, for a type finder.
    pub widget: Option<WidgetId>,
    /// The node's role, for a semantics finder.
    pub role: Option<Role>,
    /// The node's label, or its value when it has no label, for a semantics finder; for
    /// [`text`], the string that matched.
    pub text: Option<String>,
    /// The node's or widget's bounds in window coordinates.
    pub bounds: Rect,
}

/// What [`Tester::perform_action`] acts on: a semantics node, by id or by [`Finder`].
#[derive(Clone, Debug, PartialEq)]
pub enum ActionTarget {
    /// This node.
    Node(NodeId),
    /// The first node this semantics finder matches.
    Finder(Finder),
}

impl From<NodeId> for ActionTarget {
    fn from(node: NodeId) -> Self {
        ActionTarget::Node(node)
    }
}

impl From<Finder> for ActionTarget {
    fn from(finder: Finder) -> Self {
        ActionTarget::Finder(finder)
    }
}

impl From<&Finder> for ActionTarget {
    fn from(finder: &Finder) -> Self {
        ActionTarget::Finder(finder.clone())
    }
}

/// The semantics nodes in tree order (pre-order from the window root), then any node the walk
/// did not reach, in collection order.
fn nodes_in_tree_order(update: &SemanticsUpdate) -> Vec<(NodeId, &Node)> {
    let by_id: HashMap<NodeId, &Node> = update.nodes.iter().map(|(id, n)| (*id, n)).collect();
    let mut seen = HashSet::new();
    let mut ordered = Vec::with_capacity(update.nodes.len());
    let mut stack = vec![update.root];
    while let Some(id) = stack.pop() {
        let Some(node) = by_id.get(&id) else { continue };
        if !seen.insert(id) {
            continue;
        }
        ordered.push((id, *node));
        stack.extend(node.children().iter().rev().copied());
    }
    for (id, node) in &update.nodes {
        if seen.insert(*id) {
            ordered.push((*id, node));
        }
    }
    ordered
}

fn node_bounds(node: &Node) -> Rect {
    node.bounds()
        .map_or(Rect::ZERO, |r| Rect::new(r.x0, r.y0, r.x1, r.y1))
}

impl<S: 'static, V: View<S>> Tester<S, V> {
    /// Every node `finder` matches, in tree order, as of the last layout.
    pub fn find(&self, finder: &Finder) -> Vec<Match> {
        if let FinderKind::Type(name) = &finder.0 {
            return self
                .inspect()
                .into_iter()
                .filter(|node| node.type_name == name)
                .map(|node| Match {
                    node: None,
                    widget: Some(node.id),
                    role: None,
                    text: None,
                    bounds: node.bounds,
                })
                .collect();
        }
        let update = self.semantics();
        nodes_in_tree_order(&update)
            .into_iter()
            .filter_map(|(id, node)| {
                finder.match_node(node).map(|text| Match {
                    node: Some(id),
                    widget: None,
                    role: Some(node.role()),
                    text,
                    bounds: node_bounds(node),
                })
            })
            .collect()
    }

    /// The first node `finder` matches (Flutter's `getSemantics`).
    ///
    /// # Panics
    ///
    /// When nothing matches. The message lists every node that carries text as `role: text`, or
    /// for a type finder every widget type in the tree.
    pub fn get_semantics(&self, finder: &Finder) -> Match {
        match self.find(finder).into_iter().next() {
            Some(found) => found,
            None => panic!("{}", self.miss_message(finder)),
        }
    }

    /// The window-space bounds of the first node `finder` matches.
    ///
    /// # Panics
    ///
    /// When nothing matches, as [`get_semantics`](Self::get_semantics) does.
    pub fn bounds(&self, finder: &Finder) -> Rect {
        self.get_semantics(finder).bounds
    }

    /// Every label and every value in the semantics tree, in tree order: what the screen shows
    /// as text, for an `assert!(tester.texts().contains(..))` or a failure message.
    pub fn texts(&self) -> Vec<String> {
        let update = self.semantics();
        nodes_in_tree_order(&update)
            .into_iter()
            .flat_map(|(_, node)| [node.label(), node.value()])
            .flatten()
            .map(str::to_owned)
            .collect()
    }

    /// The node an [`ActionTarget`] names.
    ///
    /// # Panics
    ///
    /// When a finder matches nothing, or is a type finder (widgets have no node id).
    pub(super) fn resolve_node(&self, target: ActionTarget) -> NodeId {
        match target {
            ActionTarget::Node(node) => node,
            ActionTarget::Finder(finder) => {
                assert!(
                    finder.is_semantic(),
                    "perform_action needs a semantics finder; {finder} matches widgets, not nodes"
                );
                self.get_semantics(&finder)
                    .node
                    .expect("a semantics match carries its node")
            }
        }
    }

    /// The failure message for a `finder` that matched nothing.
    pub(super) fn miss_message(&self, finder: &Finder) -> String {
        let mut message = format!("no node matches {finder}; ");
        if finder.is_semantic() {
            message.push_str("nodes with text:");
            let update = self.semantics();
            for (_, node) in nodes_in_tree_order(&update) {
                for text in [node.label(), node.value()].into_iter().flatten() {
                    message.push_str(&format!("\n  {:?}: {text}", node.role()));
                }
            }
        } else {
            message.push_str("widget types:");
            for node in self.inspect() {
                message.push_str(&format!("\n  {}", node.type_name));
            }
        }
        message
    }
}

#[cfg(test)]
mod tests {
    use kurbo::Size;

    use crate::authoring::{Action, Role, TextWidget};
    use crate::testing::{Tester, find};

    const SIZE: Size = Size::new(300.0, 200.0);

    #[derive(Default)]
    struct Saved {
        saves: u32,
        text: String,
    }

    fn save_button(_: &mut Saved) -> crate::ButtonView<Saved> {
        crate::button("Save", |state: &mut Saved| state.saves += 1)
    }

    #[test]
    fn tap_on_a_button_finder_runs_its_on_press() {
        let mut tester = Tester::new(Saved::default(), save_button, SIZE);
        tester.tap(find::label(Role::Button, "Save"));
        assert_eq!(tester.state().saves, 1);
    }

    #[test]
    fn perform_action_click_on_a_finder_runs_on_press() {
        let mut tester = Tester::new(Saved::default(), save_button, SIZE);
        let save = find::label(Role::Button, "Save");
        let outcome = tester.perform_action(&save, Action::Click);
        assert!(outcome.handled);
        assert_eq!(tester.state().saves, 1);
        assert_eq!(tester.find(&save).len(), 1, "the finder is reusable");

        let node = tester
            .get_semantics(&find::role(Role::Button))
            .node
            .expect("a semantics match has a node");
        tester.perform_action(node, Action::Click);
        assert_eq!(tester.state().saves, 2, "the NodeId form still works");
    }

    #[test]
    #[should_panic(expected = "Button: Save")]
    fn get_semantics_on_a_miss_lists_the_labelled_nodes() {
        let tester = Tester::new(Saved::default(), save_button, SIZE);
        tester.get_semantics(&find::text("nope"));
    }

    #[test]
    fn by_type_finds_the_label_text_widget_inside_a_button() {
        let tester = Tester::new(Saved::default(), save_button, SIZE);
        let button = tester.bounds(&find::label(Role::Button, "Save"));
        let labels = tester.find(&find::by_type::<TextWidget>());
        assert_eq!(labels.len(), 1, "one text pod, the button's label");
        let label = &labels[0];
        assert!(label.widget.is_some() && label.node.is_none());
        assert!(label.bounds.width() > 0.0 && label.bounds.height() > 0.0);
        assert!(
            button.contains(label.bounds.origin())
                && label.bounds.x1 <= button.x1
                && label.bounds.y1 <= button.y1,
            "label {:?} sits inside the button {button:?}",
            label.bounds
        );
        assert!(label.bounds.area() < button.area());

        // The semantics tree has no separate node for the label: "Save" is the button's own.
        let semantic = tester.find(&find::text("Save"));
        assert_eq!(semantic.len(), 1);
        assert_eq!(semantic[0].role, Some(Role::Button));

        let by_name = tester.find(&find::by_type_name(core::any::type_name::<TextWidget>()));
        assert_eq!(by_name, labels);
    }

    #[test]
    fn text_and_value_find_a_text_fields_contents_and_label_text_does_not() {
        let tester = Tester::new(
            Saved {
                saves: 0,
                text: "draft".to_owned(),
            },
            |state: &mut Saved| {
                crate::text_input(state.text.clone(), |state: &mut Saved, text| {
                    state.text = text;
                })
            },
            SIZE,
        );
        assert_eq!(tester.find(&find::text("draft")).len(), 1);
        let by_value = tester.find(&find::value("draft"));
        assert_eq!(by_value.len(), 1);
        assert_eq!(by_value[0].role, Some(Role::TextInput));
        assert!(tester.find(&find::label_text("draft")).is_empty());
        assert!(tester.texts().contains(&"draft".to_owned()));
    }

    #[test]
    fn texts_lists_labels_and_values_in_tree_order() {
        let tester = Tester::new(
            Saved::default(),
            |_: &mut Saved| {
                crate::Column((
                    crate::text("first"),
                    crate::button("Save", |state: &mut Saved| state.saves += 1),
                    crate::text("last"),
                ))
            },
            SIZE,
        );
        let texts = tester.texts();
        let at = |s: &str| texts.iter().position(|t| t == s).expect(s);
        assert!(
            at("first") < at("Save") && at("Save") < at("last"),
            "{texts:?}"
        );
        assert_eq!(tester.find(&find::role(Role::Button)).len(), 1);
        assert_eq!(
            tester.get_semantics(&find::text("first")).role,
            Some(Role::Label)
        );
    }

    #[test]
    #[should_panic(expected = "widget types:")]
    fn a_type_finder_miss_lists_the_widget_types() {
        let tester = Tester::new(Saved::default(), save_button, SIZE);
        tester.get_semantics(&find::by_type_name("NoSuchWidget"));
    }
}
