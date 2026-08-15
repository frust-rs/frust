//! The native menu bar: [`MenuSpec`] → a Win32 `HMENU` on the window's own
//! handle, and the per-frame drain that turns an activation back into a
//! `frust_reactive` menu event.
//!
//! # A plan first, muda second
//!
//! The translation runs in two stages. [`MenuPlan::from_spec`] is pure,
//! platform-free, and compiles everywhere: it resolves each [`MenuItemSpec`]
//! into what Windows can actually show — dropping the roles this platform has
//! no notion of — and collects the set of ids that may be reported as
//! activations. Only [`install`] touches `muda`, and only on Windows. That
//! split is what makes the mapping (the part with actual decisions in it)
//! unit-testable on the Linux dev host, where the whole native half is
//! compiled out.
//!
//! # Accelerators are carried, not parsed, by the plan
//!
//! An accelerator stays the app's own `"CmdOrCtrl+Shift+P"` string all the way
//! into `muda::accelerator::Accelerator::from_str`, whose vocabulary is
//! precisely the one [`MenuItemSpec::Item::accelerator`] documents. Validating
//! it here as well would be a second grammar that could disagree with the
//! backend's — the config type's own docs make the backend the single
//! validator. An unparseable accelerator therefore degrades to an item with no
//! keyboard shortcut, logged, never to a menu that fails to install.

use std::collections::HashSet;

use frust_shell_desktop::config::{MenuItemSpec, MenuRole, MenuSpec};

use crate::win32_glue::AcceleratorTable;

/// The [`MenuRole`]s Windows implements. `MenuRole::ShowAll` has no member: it
/// is a macOS-only notion (muda documents it unsupported on Windows), so it is
/// dropped from the plan rather than faked with an item that does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlannedRole {
    About,
    Hide,
    HideOthers,
    Minimize,
    CloseWindow,
    Quit,
}

impl PlannedRole {
    /// The Windows equivalent of `role`, or `None` when this platform has none.
    fn for_role(role: MenuRole) -> Option<Self> {
        match role {
            MenuRole::About => Some(Self::About),
            MenuRole::Hide => Some(Self::Hide),
            MenuRole::HideOthers => Some(Self::HideOthers),
            MenuRole::Minimize => Some(Self::Minimize),
            MenuRole::CloseWindow => Some(Self::CloseWindow),
            MenuRole::Quit => Some(Self::Quit),
            MenuRole::ShowAll => None,
        }
    }
}

/// One entry of a planned menu level: a [`MenuItemSpec`] that survived the
/// mapping above.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlannedItem {
    /// An app item, reported through `frust_reactive::push_menu_event` by
    /// [`pump_menu_events`] when activated.
    Item {
        id: String,
        label: String,
        /// The app's accelerator shorthand, still unparsed (see the module
        /// docs).
        accelerator: Option<String>,
        enabled: bool,
    },
    /// A platform-implemented item. Reports no activation.
    Role {
        role: PlannedRole,
        /// An override for the platform's own label.
        label: Option<String>,
    },
    Separator,
    Submenu {
        label: String,
        items: Vec<PlannedItem>,
    },
}

/// A whole menu bar, planned: the item tree plus the set of ids that may be
/// reported as activations.
///
/// The id set is derived once here rather than re-walked per activation, and
/// is what keeps muda's own auto-assigned ids (every predefined item has one)
/// from ever reaching app code as a menu event.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MenuPlan {
    items: Vec<PlannedItem>,
    activation_ids: HashSet<String>,
}

impl MenuPlan {
    /// Plan the configured menu bar. A missing or empty spec plans to an empty
    /// menu, which [`install`] declines to install at all (a bare menu strip
    /// on a window that asked for no menu is worse than none).
    pub(crate) fn from_spec(spec: Option<&MenuSpec>) -> Self {
        let items = spec.map(|spec| plan_items(&spec.items)).unwrap_or_default();
        let mut activation_ids = HashSet::new();
        collect_activation_ids(&items, &mut activation_ids);
        Self {
            items,
            activation_ids,
        }
    }

    /// Whether there is nothing to install.
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The planned top-level items.
    pub(crate) fn items(&self) -> &[PlannedItem] {
        &self.items
    }

    /// Whether `id` belongs to an app item of this menu — i.e. whether an
    /// activation carrying it should reach app code.
    pub(crate) fn activates(&self, id: &str) -> bool {
        self.activation_ids.contains(id)
    }
}

/// Map one level of a [`MenuSpec`], dropping what Windows cannot show.
fn plan_items(items: &[MenuItemSpec]) -> Vec<PlannedItem> {
    items
        .iter()
        .filter_map(|item| match item {
            MenuItemSpec::Item {
                id,
                label,
                accelerator,
                enabled,
            } => Some(PlannedItem::Item {
                id: id.clone(),
                label: label.clone(),
                accelerator: accelerator.clone(),
                enabled: *enabled,
            }),
            MenuItemSpec::Role { role, label } => match PlannedRole::for_role(*role) {
                Some(role) => Some(PlannedItem::Role {
                    role,
                    label: label.clone(),
                }),
                None => {
                    log::debug!(
                        "frust-shell-windows: dropping menu role {role:?} — Windows has no equivalent"
                    );
                    None
                }
            },
            MenuItemSpec::Separator => Some(PlannedItem::Separator),
            MenuItemSpec::Submenu { label, menu } => Some(PlannedItem::Submenu {
                label: label.clone(),
                items: plan_items(&menu.items),
            }),
        })
        .collect()
}

/// Walk the planned tree, gathering every app item's id.
fn collect_activation_ids(items: &[PlannedItem], ids: &mut HashSet<String>) {
    for item in items {
        match item {
            PlannedItem::Item { id, .. } => {
                ids.insert(id.clone());
            }
            PlannedItem::Submenu { items, .. } => collect_activation_ids(items, ids),
            PlannedItem::Role { .. } | PlannedItem::Separator => {}
        }
    }
}

/// A menu bar attached to a live window, retained for as long as the app runs.
///
/// Holding it is load-bearing twice over: muda's `HMENU` and its accelerator
/// table both live exactly as long as the `Menu`, so dropping this un-menus the
/// window — and drops the `HACCEL` the message hook translates against, which
/// is why [`Drop`] clears the shared [`AcceleratorTable`].
#[cfg(target_os = "windows")]
pub(crate) struct InstalledMenu {
    menu: muda::Menu,
    accelerators: AcceleratorTable,
}

#[cfg(target_os = "windows")]
impl InstalledMenu {
    /// The menu's accelerator table handle.
    fn haccel(&self) -> isize {
        self.menu.haccel()
    }
}

#[cfg(target_os = "windows")]
impl Drop for InstalledMenu {
    fn drop(&mut self) {
        // The `HACCEL` dies with `self.menu`; the message hook outlives both.
        self.accelerators.clear();
    }
}

/// The off-Windows stand-in. Never constructed — [`install`] refuses there —
/// but named, so the extension's field type needs no `cfg` of its own.
#[cfg(not(target_os = "windows"))]
pub(crate) struct InstalledMenu {}

/// Build the planned menu, attach it to `window`'s `HWND`, and publish its
/// accelerator table to the message hook.
///
/// `None` — logged at each refusal point — whenever there is no menu to
/// install or Windows would not take it; the app then runs menu-less rather
/// than not at all.
#[cfg(target_os = "windows")]
pub(crate) fn install(
    plan: &MenuPlan,
    window: &winit::window::Window,
    accelerators: &AcceleratorTable,
) -> Option<InstalledMenu> {
    if plan.is_empty() {
        return None;
    }
    let hwnd = crate::win32_glue::window_hwnd(window)?;
    let menu = match build_menu(plan) {
        Ok(menu) => menu,
        Err(err) => {
            log::warn!("frust-shell-windows: could not build the native menu bar: {err}");
            return None;
        }
    };
    if !crate::win32_glue::attach_menu_to_hwnd(&menu, hwnd) {
        return None;
    }
    let installed = InstalledMenu {
        menu,
        accelerators: accelerators.clone(),
    };
    // Only now, with the menu attached and retained, does the hook get a table
    // to translate against.
    accelerators.set(installed.haccel());
    Some(installed)
}

// Inert off Windows: there is no native menu bar to install.
#[cfg(not(target_os = "windows"))]
pub(crate) fn install(
    plan: &MenuPlan,
    window: &winit::window::Window,
    accelerators: &AcceleratorTable,
) -> Option<InstalledMenu> {
    let _ = (plan, window, accelerators);
    None
}

/// Turn the plan into a real muda menu tree.
#[cfg(target_os = "windows")]
fn build_menu(plan: &MenuPlan) -> muda::Result<muda::Menu> {
    let menu = muda::Menu::new();
    append_items(&menu, plan.items())?;
    Ok(menu)
}

/// The two things a planned item can be appended to. muda gives `Menu` and
/// `Submenu` the same `append` shape without a shared trait, so this is it.
#[cfg(target_os = "windows")]
trait MenuContainer {
    fn append_item(&self, item: &dyn muda::IsMenuItem) -> muda::Result<()>;
}

#[cfg(target_os = "windows")]
impl MenuContainer for muda::Menu {
    fn append_item(&self, item: &dyn muda::IsMenuItem) -> muda::Result<()> {
        self.append(item)
    }
}

#[cfg(target_os = "windows")]
impl MenuContainer for muda::Submenu {
    fn append_item(&self, item: &dyn muda::IsMenuItem) -> muda::Result<()> {
        self.append(item)
    }
}

#[cfg(target_os = "windows")]
fn append_items(target: &dyn MenuContainer, items: &[PlannedItem]) -> muda::Result<()> {
    for item in items {
        match item {
            PlannedItem::Item {
                id,
                label,
                accelerator,
                enabled,
            } => {
                let accelerator = accelerator.as_deref().and_then(parse_accelerator);
                target.append_item(&muda::MenuItem::with_id(
                    id.as_str(),
                    label,
                    *enabled,
                    accelerator,
                ))?;
            }
            PlannedItem::Role { role, label } => {
                target.append_item(&predefined(*role, label.as_deref()))?;
            }
            PlannedItem::Separator => {
                target.append_item(&muda::PredefinedMenuItem::separator())?;
            }
            PlannedItem::Submenu { label, items } => {
                let submenu = muda::Submenu::new(label, true);
                append_items(&submenu, items)?;
                target.append_item(&submenu)?;
            }
        }
    }
    Ok(())
}

/// Parse an app accelerator string, or `None` (logged) if muda will not take
/// it — the documented degrade: the item still appears, without a shortcut.
#[cfg(target_os = "windows")]
fn parse_accelerator(accelerator: &str) -> Option<muda::accelerator::Accelerator> {
    use std::str::FromStr;

    match muda::accelerator::Accelerator::from_str(accelerator) {
        Ok(parsed) => Some(parsed),
        Err(err) => {
            log::warn!(
                "frust-shell-windows: ignoring unparseable menu accelerator {accelerator:?}: {err}"
            );
            None
        }
    }
}

#[cfg(target_os = "windows")]
fn predefined(role: PlannedRole, label: Option<&str>) -> muda::PredefinedMenuItem {
    match role {
        PlannedRole::About => muda::PredefinedMenuItem::about(label, None),
        PlannedRole::Hide => muda::PredefinedMenuItem::hide(label),
        PlannedRole::HideOthers => muda::PredefinedMenuItem::hide_others(label),
        PlannedRole::Minimize => muda::PredefinedMenuItem::minimize(label),
        PlannedRole::CloseWindow => muda::PredefinedMenuItem::close_window(label),
        PlannedRole::Quit => muda::PredefinedMenuItem::quit(label),
    }
}

/// Drain the platform menu queue into `frust_reactive`, once per frame.
///
/// Both routes into it land here: a click on the menu, and a keystroke the
/// message hook turned into a menu command via `TranslateAcceleratorW`. muda
/// reports predefined items too (it assigns each its own id); those are the
/// platform's own actions with nothing for app code to handle, so only ids the
/// plan knows are forwarded.
#[cfg(target_os = "windows")]
pub(crate) fn pump_menu_events(plan: &MenuPlan) {
    while let Ok(event) = muda::MenuEvent::receiver().try_recv() {
        let id: &str = event.id().as_ref();
        if plan.activates(id) {
            frust_reactive::push_menu_event(id);
        } else {
            log::trace!("frust-shell-windows: ignoring non-app menu event {id:?}");
        }
    }
}

// Inert off Windows: no platform menu queue exists to drain.
#[cfg(not(target_os = "windows"))]
pub(crate) fn pump_menu_events(plan: &MenuPlan) {
    let _ = plan;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, label: &str) -> MenuItemSpec {
        MenuItemSpec::item(id, label)
    }

    #[test]
    fn no_spec_plans_to_an_empty_menu() {
        let plan = MenuPlan::from_spec(None);
        assert!(plan.is_empty());
        assert!(plan.items().is_empty());
    }

    #[test]
    fn an_empty_spec_plans_to_an_empty_menu() {
        // The "install nothing rather than a bare strip" case.
        assert!(MenuPlan::from_spec(Some(&MenuSpec::new())).is_empty());
    }

    #[test]
    fn an_app_item_keeps_its_id_label_accelerator_and_enabled_state() {
        let spec = MenuSpec::new()
            .with_item(item("file.open", "Open…").with_accelerator("CmdOrCtrl+O"))
            .with_item(item("file.export", "Export").disabled());
        let plan = MenuPlan::from_spec(Some(&spec));

        assert_eq!(
            plan.items(),
            [
                PlannedItem::Item {
                    id: "file.open".to_string(),
                    label: "Open…".to_string(),
                    // Carried verbatim — muda's parser is the only validator.
                    accelerator: Some("CmdOrCtrl+O".to_string()),
                    enabled: true,
                },
                PlannedItem::Item {
                    id: "file.export".to_string(),
                    label: "Export".to_string(),
                    accelerator: None,
                    enabled: false,
                },
            ]
        );
    }

    #[test]
    fn a_submenu_is_planned_recursively() {
        let file = MenuSpec::new()
            .with_item(item("file.open", "Open…"))
            .with_item(MenuItemSpec::separator())
            .with_item(MenuItemSpec::role(MenuRole::Quit));
        let spec = MenuSpec::new().with_item(MenuItemSpec::submenu("File", file));
        let plan = MenuPlan::from_spec(Some(&spec));

        assert_eq!(
            plan.items(),
            [PlannedItem::Submenu {
                label: "File".to_string(),
                items: vec![
                    PlannedItem::Item {
                        id: "file.open".to_string(),
                        label: "Open…".to_string(),
                        accelerator: None,
                        enabled: true,
                    },
                    PlannedItem::Separator,
                    PlannedItem::Role {
                        role: PlannedRole::Quit,
                        label: None,
                    },
                ],
            }]
        );
    }

    #[test]
    fn every_role_windows_implements_survives_planning() {
        let spec = MenuSpec::new()
            .with_item(MenuItemSpec::role(MenuRole::About))
            .with_item(MenuItemSpec::role(MenuRole::Hide))
            .with_item(MenuItemSpec::role(MenuRole::HideOthers))
            .with_item(MenuItemSpec::role(MenuRole::Minimize))
            .with_item(MenuItemSpec::role(MenuRole::CloseWindow))
            .with_item(MenuItemSpec::role(MenuRole::Quit));
        let plan = MenuPlan::from_spec(Some(&spec));

        let roles: Vec<PlannedRole> = plan
            .items()
            .iter()
            .map(|item| match item {
                PlannedItem::Role { role, .. } => *role,
                other => panic!("expected a role item, got {other:?}"),
            })
            .collect();
        assert_eq!(
            roles,
            vec![
                PlannedRole::About,
                PlannedRole::Hide,
                PlannedRole::HideOthers,
                PlannedRole::Minimize,
                PlannedRole::CloseWindow,
                PlannedRole::Quit,
            ]
        );
    }

    #[test]
    fn a_role_windows_does_not_implement_is_dropped_not_faked() {
        let spec = MenuSpec::new()
            .with_item(MenuItemSpec::role(MenuRole::ShowAll))
            .with_item(item("view.reload", "Reload"));
        let plan = MenuPlan::from_spec(Some(&spec));

        assert_eq!(plan.items().len(), 1);
        assert!(matches!(plan.items()[0], PlannedItem::Item { .. }));
    }

    #[test]
    fn a_role_keeps_its_label_override() {
        let spec =
            MenuSpec::new().with_item(MenuItemSpec::role(MenuRole::About).with_label("About Fake"));
        let plan = MenuPlan::from_spec(Some(&spec));
        assert_eq!(
            plan.items(),
            [PlannedItem::Role {
                role: PlannedRole::About,
                label: Some("About Fake".to_string()),
            }]
        );
    }

    #[test]
    fn only_app_item_ids_are_activatable_at_any_depth() {
        let edit = MenuSpec::new().with_item(item("edit.undo", "Undo"));
        let file = MenuSpec::new()
            .with_item(item("file.open", "Open…"))
            .with_item(MenuItemSpec::role(MenuRole::Quit))
            .with_item(MenuItemSpec::submenu("Edit", edit));
        let plan = MenuPlan::from_spec(Some(
            &MenuSpec::new().with_item(MenuItemSpec::submenu("File", file)),
        ));

        assert!(plan.activates("file.open"));
        // Nested two levels down — the drain must still forward it.
        assert!(plan.activates("edit.undo"));
        // muda assigns predefined items ids of their own; none of them is the
        // app's, so none is ever forwarded.
        assert!(!plan.activates("File"));
        assert!(!plan.activates("1000"));
        assert!(!plan.activates(""));
    }
}
