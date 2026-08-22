// Ported from material_3_expressive v1.0.8 (MIT, © 2026 Paa Developments),
// `lib/components/menus/models/m3e_menu_node.dart` (the sealed node vocabulary
// and `m3eMenuPartitionSurfaces`) plus `utils/m3e_menu_placer.dart`'s
// `approximateItemCount` (the depth-first walk this module's `span` mirrors),
// retrieved 2026-08-19.
// Porting decisions: the reference's `M3EMenuWidget` (an arbitrary host-built
// row body) is **not** ported — a Rust node tree is plain data, so a row's body
// is the label/icon/shortcut vocabulary below rather than a boxed child view;
// its `badge` slot is dropped for the same reason. `leading`/`trailing` narrow
// from `Widget?` to the vendored `crate::icons` set.

//! The menu content tree: the six node kinds a caller composes, and the
//! surface partitioning the panel renders them through.
//!
//! # The six kinds
//!
//! | Node | Reference | Row |
//! |---|---|---|
//! | [`MenuEntry`] | `M3EMenuEntry` | a plain action row |
//! | [`MenuSelectable`] | `M3EMenuSelectable` | radio-style: carries a value, checks when selected |
//! | [`MenuToggleable`] | `M3EMenuToggleable` | checkbox-style: carries a checked state |
//! | [`MenuNode::Divider`] | `M3EMenuDivider` | a hairline between sections of one surface |
//! | [`MenuGroup`] | `M3EMenuGroup` | a labelled section — **its own elevated surface** |
//! | [`MenuSubmenu`] | `M3EMenuSubmenu` | a row that opens a nested panel |
//!
//! # Surfaces: a group is a container, not an indent
//!
//! [`partition_surfaces`] ports `m3eMenuPartitionSurfaces` exactly: each
//! top-level [`MenuGroup`] becomes its own elevated menu container, and every
//! run of consecutive non-group nodes collapses into one implicit, unlabelled
//! surface. A menu is therefore a *stack* of cards, not one card with headings.
//!
//! # Indices
//!
//! Every **rendered row** takes one index in a depth-first, pre-order walk of
//! the tree — a divider counts (it is a row), a submenu row counts and its
//! children take the indices that follow, and a [`MenuGroup`] counts for
//! nothing of its own (it is a surface, and its label is chrome, not a row).
//! [`MenuSelection::index`] reports that number, so a caller matches on a
//! number it can read straight off its own node list. [`MenuNode::span`] is how
//! many indices a node and its whole subtree consume (the same shape
//! `frust_shadcn`'s menu list uses for its own numbering).

use frust::IconSource;

/// An icon slot on a menu row.
///
/// A thin wrapper over [`frust::IconSource`] that adds the `PartialEq` the
/// generated constants do not carry, so a node tree can be compared across a
/// rebuild. Build one from any `crate::icons` constant — every slot below takes
/// `impl Into<MenuIcon>`, so `icons::CONTENT_COPY` passes straight in.
#[derive(Clone, Copy, Debug)]
pub struct MenuIcon(pub IconSource);

impl PartialEq for MenuIcon {
    fn eq(&self, other: &Self) -> bool {
        // `IconSource` is `{ d: &'static str, design: f64 }`; two slots holding
        // the same path in the same design box paint identically, which is the
        // only equality a rebuild diff needs.
        self.0.d == other.0.d && self.0.design == other.0.design
    }
}

impl From<IconSource> for MenuIcon {
    fn from(source: IconSource) -> Self {
        MenuIcon(source)
    }
}

/// One node in a menu content tree. See the [module docs](self).
#[derive(Clone, Debug, PartialEq)]
pub enum MenuNode {
    /// A plain action row.
    Entry(MenuEntry),
    /// A radio-style row carrying a value.
    Selectable(MenuSelectable),
    /// A checkbox-style row carrying a checked state.
    Toggleable(MenuToggleable),
    /// A hairline rule between sections of one surface.
    Divider,
    /// A labelled section, rendered as its own elevated surface.
    Group(MenuGroup),
    /// A row that opens a nested panel.
    Submenu(MenuSubmenu),
}

impl MenuNode {
    /// How many indices this node and its whole subtree consume in the
    /// depth-first numbering (see the [module docs](self)).
    pub fn span(&self) -> usize {
        match self {
            // A group renders no row of its own — only a surface holding its
            // children's.
            MenuNode::Group(group) => spans(&group.children),
            MenuNode::Submenu(submenu) => 1 + spans(&submenu.children),
            _ => 1,
        }
    }

    /// Whether this node can be activated at all — a divider and a group never
    /// are, and a disabled row refuses.
    pub fn is_activatable(&self) -> bool {
        match self {
            MenuNode::Entry(e) => e.enabled,
            MenuNode::Selectable(e) => e.enabled,
            MenuNode::Toggleable(e) => e.enabled,
            MenuNode::Submenu(e) => e.enabled,
            MenuNode::Divider | MenuNode::Group(_) => false,
        }
    }
}

/// The total span of `nodes` — their count plus every nested subtree's.
pub fn spans(nodes: &[MenuNode]) -> usize {
    nodes.iter().map(MenuNode::span).sum()
}

/// A plain action row (`M3EMenuEntry`). Build one with [`menu_entry`].
#[derive(Clone, Debug, PartialEq)]
pub struct MenuEntry {
    pub(super) label: String,
    pub(super) leading: Option<MenuIcon>,
    pub(super) trailing: Option<MenuIcon>,
    pub(super) shortcut: Option<String>,
    pub(super) supporting: Option<String>,
    pub(super) enabled: bool,
    pub(super) destructive: bool,
}

/// A plain action row labelled `label`.
pub fn menu_entry(label: impl Into<String>) -> MenuEntry {
    MenuEntry {
        label: label.into(),
        leading: None,
        trailing: None,
        shortcut: None,
        supporting: None,
        enabled: true,
        destructive: false,
    }
}

impl MenuEntry {
    /// Set the leading icon.
    pub fn leading(mut self, icon: impl Into<MenuIcon>) -> Self {
        self.leading = Some(icon.into());
        self
    }

    /// Set the trailing icon (drawn after the shortcut text, per the
    /// reference's row order).
    pub fn trailing(mut self, icon: impl Into<MenuIcon>) -> Self {
        self.trailing = Some(icon.into());
        self
    }

    /// Set the trailing shortcut hint (`trailingText`, e.g. `⌘C`).
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Set the second line under the label (`supportingText`).
    pub fn supporting(mut self, supporting: impl Into<String>) -> Self {
        self.supporting = Some(supporting.into());
        self
    }

    /// Enable or disable the row. Enabled by default.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Paint the row's label and icons in the `error` role (`isDestructive`).
    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// The row's label — also its accessible name.
    pub fn label(&self) -> &str {
        &self.label
    }
}

impl From<MenuEntry> for MenuNode {
    fn from(entry: MenuEntry) -> Self {
        MenuNode::Entry(entry)
    }
}

/// A radio-style row carrying a value (`M3EMenuSelectable`). Build one with
/// [`menu_selectable`].
#[derive(Clone, Debug, PartialEq)]
pub struct MenuSelectable {
    pub(super) label: String,
    pub(super) value: String,
    pub(super) leading: Option<MenuIcon>,
    pub(super) trailing: Option<MenuIcon>,
    pub(super) shortcut: Option<String>,
    pub(super) supporting: Option<String>,
    pub(super) selected: bool,
    pub(super) enabled: bool,
}

/// A radio-style row labelled `label`, reporting `value` when chosen.
///
/// Controlled, like every selection control in this catalog: the row reports
/// the *requested* value and never flips its own `selected` — hand the panel
/// the confirmed value through
/// [`MenuPanelView::selected`](super::MenuPanelView::selected) (or
/// [`MenuView::selected`](super::MenuView::selected)) on the next rebuild.
pub fn menu_selectable(label: impl Into<String>, value: impl Into<String>) -> MenuSelectable {
    MenuSelectable {
        label: label.into(),
        value: value.into(),
        leading: None,
        trailing: None,
        shortcut: None,
        supporting: None,
        selected: false,
        enabled: true,
    }
}

impl MenuSelectable {
    /// Set the leading icon. Without one, a selected row takes the reference's
    /// `M3EIcons.check_rounded` (`m3e_menu_node_builders.dart:55`).
    pub fn leading(mut self, icon: impl Into<MenuIcon>) -> Self {
        self.leading = Some(icon.into());
        self
    }

    /// Set the trailing icon.
    pub fn trailing(mut self, icon: impl Into<MenuIcon>) -> Self {
        self.trailing = Some(icon.into());
        self
    }

    /// Set the trailing shortcut hint.
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Set the second line under the label.
    pub fn supporting(mut self, supporting: impl Into<String>) -> Self {
        self.supporting = Some(supporting.into());
        self
    }

    /// Mark the row selected outright, independent of the panel's own selected
    /// value (the reference's per-item `selected` flag, OR-ed with the value
    /// match — `m3e_menu_content.dart:184`).
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Enable or disable the row. Enabled by default.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The row's label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The value this row reports when chosen.
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl From<MenuSelectable> for MenuNode {
    fn from(item: MenuSelectable) -> Self {
        MenuNode::Selectable(item)
    }
}

/// A checkbox-style row (`M3EMenuToggleable`). Build one with
/// [`menu_toggleable`].
#[derive(Clone, Debug, PartialEq)]
pub struct MenuToggleable {
    pub(super) label: String,
    pub(super) checked: bool,
    pub(super) leading: Option<MenuIcon>,
    pub(super) trailing: Option<MenuIcon>,
    pub(super) shortcut: Option<String>,
    pub(super) supporting: Option<String>,
    pub(super) enabled: bool,
}

/// A checkbox-style row labelled `label`, currently `checked`.
///
/// Controlled: activating it reports the *requested* state (`!checked`) and
/// leaves `checked` untouched until the caller feeds the confirmed value back
/// down on the next rebuild.
pub fn menu_toggleable(label: impl Into<String>, checked: bool) -> MenuToggleable {
    MenuToggleable {
        label: label.into(),
        checked,
        leading: None,
        trailing: None,
        shortcut: None,
        supporting: None,
        enabled: true,
    }
}

impl MenuToggleable {
    /// Set the leading icon. Without one, the row takes the reference's
    /// `check_box_rounded`/`check_box_outline_blank_rounded` pair
    /// (`m3e_menu_node_builders.dart:88`).
    pub fn leading(mut self, icon: impl Into<MenuIcon>) -> Self {
        self.leading = Some(icon.into());
        self
    }

    /// Set the trailing icon.
    pub fn trailing(mut self, icon: impl Into<MenuIcon>) -> Self {
        self.trailing = Some(icon.into());
        self
    }

    /// Set the trailing shortcut hint.
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Set the second line under the label.
    pub fn supporting(mut self, supporting: impl Into<String>) -> Self {
        self.supporting = Some(supporting.into());
        self
    }

    /// Enable or disable the row. Enabled by default.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The row's label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Whether the row currently reads as checked.
    pub fn checked(&self) -> bool {
        self.checked
    }
}

impl From<MenuToggleable> for MenuNode {
    fn from(item: MenuToggleable) -> Self {
        MenuNode::Toggleable(item)
    }
}

/// A labelled section rendered as its own elevated surface (`M3EMenuGroup`).
/// Build one with [`menu_group`].
#[derive(Clone, Debug, PartialEq)]
pub struct MenuGroup {
    pub(super) label: Option<String>,
    pub(super) children: Vec<MenuNode>,
}

/// A section holding `children`, unlabelled until [`MenuGroup::label`].
pub fn menu_group(children: Vec<MenuNode>) -> MenuGroup {
    MenuGroup {
        label: None,
        children,
    }
}

impl MenuGroup {
    /// Set the section header drawn above the group's rows.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The group's rows.
    pub fn children(&self) -> &[MenuNode] {
        &self.children
    }

    /// The section header, if any.
    pub fn section_label(&self) -> Option<&str> {
        self.label.as_deref()
    }
}

impl From<MenuGroup> for MenuNode {
    fn from(group: MenuGroup) -> Self {
        MenuNode::Group(group)
    }
}

/// A row that opens a nested panel (`M3EMenuSubmenu`). Build one with
/// [`menu_submenu`].
#[derive(Clone, Debug, PartialEq)]
pub struct MenuSubmenu {
    pub(super) label: String,
    pub(super) leading: Option<MenuIcon>,
    pub(super) children: Vec<MenuNode>,
    pub(super) enabled: bool,
}

/// A row labelled `label` opening a nested panel over `children`.
pub fn menu_submenu(label: impl Into<String>, children: Vec<MenuNode>) -> MenuSubmenu {
    MenuSubmenu {
        label: label.into(),
        leading: None,
        children,
        enabled: true,
    }
}

impl MenuSubmenu {
    /// Set the leading icon. The trailing slot is always the reference's
    /// `arrow_right_rounded` chevron (`m3e_menu_node_builders.dart:128`).
    pub fn leading(mut self, icon: impl Into<MenuIcon>) -> Self {
        self.leading = Some(icon.into());
        self
    }

    /// Enable or disable the row. A disabled submenu row opens nothing.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The row's label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The nested panel's own nodes.
    pub fn children(&self) -> &[MenuNode] {
        &self.children
    }
}

impl From<MenuSubmenu> for MenuNode {
    fn from(submenu: MenuSubmenu) -> Self {
        MenuNode::Submenu(submenu)
    }
}

/// What an activated row asks the app to do.
///
/// Every kind is a *request*: the panel never mutates its own nodes, so an app
/// that ignores one simply leaves the menu as it was (this catalog's
/// controlled-component rule).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAction {
    /// A [`MenuEntry`] fired.
    Press,
    /// A [`MenuSelectable`] asks to become its group's selected value.
    Select(String),
    /// A [`MenuToggleable`] asks to flip to this checked state.
    Toggle(bool),
}

/// One activation reported through a panel's `on_select`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuSelection {
    /// The activated node's depth-first index (see the [module docs](self)).
    pub index: usize,
    /// The row's label, so a caller can match on text rather than a number.
    pub label: String,
    /// What the row requests.
    pub action: MenuAction,
}

impl MenuSelection {
    /// The value a [`MenuAction::Select`] carries, if that is what this is.
    pub fn value(&self) -> Option<&str> {
        match &self.action {
            MenuAction::Select(value) => Some(value.as_str()),
            _ => None,
        }
    }

    /// The requested state a [`MenuAction::Toggle`] carries, if that is what
    /// this is.
    pub fn checked(&self) -> Option<bool> {
        match &self.action {
            MenuAction::Toggle(checked) => Some(*checked),
            _ => None,
        }
    }
}

/// Partition top-level `nodes` into elevated surface groups — the port of
/// `m3eMenuPartitionSurfaces` (`m3e_menu_node.dart:272`).
///
/// Each [`MenuNode::Group`] becomes its own surface (keeping its label);
/// every run of consecutive non-group nodes collapses into one unlabelled
/// surface. An empty input partitions into no surfaces at all.
pub fn partition_surfaces(nodes: &[MenuNode]) -> Vec<MenuGroup> {
    let mut surfaces: Vec<MenuGroup> = Vec::new();
    let mut flat: Vec<MenuNode> = Vec::new();
    for node in nodes {
        match node {
            MenuNode::Group(group) => {
                if !flat.is_empty() {
                    surfaces.push(menu_group(std::mem::take(&mut flat)));
                }
                surfaces.push(group.clone());
            }
            other => flat.push(other.clone()),
        }
    }
    if !flat.is_empty() {
        surfaces.push(menu_group(flat));
    }
    surfaces
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icons;

    fn sample() -> Vec<MenuNode> {
        vec![
            menu_entry("Cut").shortcut("⌘X").into(),
            MenuNode::Divider,
            menu_group(vec![
                menu_selectable("Grid", "grid").into(),
                menu_selectable("List", "list").into(),
            ])
            .label("View")
            .into(),
            menu_submenu(
                "Share",
                vec![
                    menu_entry("Copy link").into(),
                    menu_toggleable("Public", true).into(),
                ],
            )
            .into(),
        ]
    }

    #[test]
    fn a_group_opens_its_own_surface_and_a_flat_run_shares_one() {
        let surfaces = partition_surfaces(&sample());
        assert_eq!(surfaces.len(), 3, "flat run, the group, then the flat tail");
        assert_eq!(surfaces[0].section_label(), None);
        assert_eq!(surfaces[0].children().len(), 2, "the entry and the divider");
        assert_eq!(surfaces[1].section_label(), Some("View"));
        assert_eq!(surfaces[1].children().len(), 2);
        assert_eq!(surfaces[2].section_label(), None);
        assert!(matches!(surfaces[2].children()[0], MenuNode::Submenu(_)));
    }

    #[test]
    fn an_empty_tree_partitions_into_no_surfaces() {
        assert!(partition_surfaces(&[]).is_empty());
    }

    #[test]
    fn consecutive_groups_never_merge() {
        let nodes = vec![
            menu_group(vec![menu_entry("a").into()]).into(),
            menu_group(vec![menu_entry("b").into()]).into(),
        ];
        assert_eq!(partition_surfaces(&nodes).len(), 2);
    }

    #[test]
    fn span_counts_the_node_plus_its_whole_subtree() {
        let nodes = sample();
        assert_eq!(nodes[0].span(), 1, "a leaf entry");
        assert_eq!(nodes[1].span(), 1, "a divider still takes an index");
        assert_eq!(nodes[2].span(), 2, "a group is a surface, not a row");
        assert_eq!(nodes[3].span(), 3, "the submenu row plus its two children");
        assert_eq!(spans(&nodes), 7);
    }

    #[test]
    fn only_enabled_leaf_rows_are_activatable() {
        assert!(MenuNode::from(menu_entry("a")).is_activatable());
        assert!(!MenuNode::from(menu_entry("a").enabled(false)).is_activatable());
        assert!(MenuNode::from(menu_selectable("a", "v")).is_activatable());
        assert!(MenuNode::from(menu_toggleable("a", false)).is_activatable());
        assert!(MenuNode::from(menu_submenu("a", vec![])).is_activatable());
        assert!(!MenuNode::from(menu_submenu("a", vec![]).enabled(false)).is_activatable());
        assert!(!MenuNode::Divider.is_activatable());
        assert!(!MenuNode::from(menu_group(vec![])).is_activatable());
    }

    #[test]
    fn an_icon_slot_compares_by_path_and_design_box() {
        let a: MenuIcon = icons::CHECK_ROUNDED.into();
        let b: MenuIcon = icons::CHECK_ROUNDED.into();
        let c: MenuIcon = icons::CONTENT_COPY.into();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(
            menu_entry("x").leading(icons::CHECK),
            menu_entry("x").leading(icons::CHECK)
        );
        assert_ne!(menu_entry("x").leading(icons::CHECK), menu_entry("x"));
    }

    #[test]
    fn a_selection_reads_its_own_payload() {
        let select = MenuSelection {
            index: 2,
            label: "Grid".into(),
            action: MenuAction::Select("grid".into()),
        };
        assert_eq!(select.value(), Some("grid"));
        assert_eq!(select.checked(), None);
        let toggle = MenuSelection {
            index: 3,
            label: "Public".into(),
            action: MenuAction::Toggle(false),
        };
        assert_eq!(toggle.checked(), Some(false));
        assert_eq!(toggle.value(), None);
    }
}
