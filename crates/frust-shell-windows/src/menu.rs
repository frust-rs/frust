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
//!
//! # Push, not poll: why activations do not go through `MenuEvent::receiver`
//!
//! `muda` publishes activations two ways: a process-global channel
//! (`MenuEvent::receiver()`) and a handler callback
//! (`MenuEvent::set_event_handler`). Polling the channel from
//! [`DesktopExtensions::pump`](frust_shell_desktop::extensions::DesktopExtensions::pump)
//! alone would lose the wake: the shared desktop core is dirty-driven
//! (`ControlFlow::Wait`) and calls `pump` only from a redraw, so an activation
//! arriving while the app is idle would sit in the channel until some unrelated
//! event happened to produce a frame. Accelerators are the sharp case — the
//! message hook that runs `TranslateAcceleratorW` reports the keystroke as
//! *handled*, so winit never sees an event of its own to wake the loop with.
//!
//! So this module installs the handler instead ([`install_event_handler`]),
//! filters the activation against the plan's ids, queues it in [`MenuBridge`],
//! and *requests a redraw* — the loop wakes, [`pump_menu_events`] takes one
//! activation off the queue at the top of that frame, and the resulting
//! `frust_reactive::push_menu_event` is visible to the very same frame's
//! rebuild.
//!
//! The queue is also what keeps the reactive push on the UI thread. `muda`'s
//! handler slot is typed `Fn(MenuEvent) + Send + Sync`, so the callback may in
//! principle run on any thread, while `push_menu_event` documents a
//! UI-thread-only contract; the handler therefore only queues and wakes (both
//! `Send`-safe — `winit::window::Window` is `Send + Sync` and
//! `request_redraw` is documented as callable from any thread), and the push
//! itself happens on the event-loop thread inside `pump`.
//!
//! # One activation per frame, then another frame
//!
//! [`MenuBridge::take_next`] hands [`pump_menu_events`] exactly *one* queued
//! activation per call and requests a further redraw while the queue is still
//! non-empty. `frust_reactive`'s menu source is a single-slot signal
//! (`MenuEvents::latest`) that a frame reads once, so pushing a whole batch
//! into it between two frames would leave only the last activation observable —
//! a burst of two menu choices, or one held accelerator, would silently
//! collapse. Frame-per-activation keeps the queue's order *and* its count
//! intact all the way to app code.
//!
//! This is the same strategy `frust-shell-macos`' `menu` module implements
//! against `muda`'s AppKit half — push bridge, known-ids filter, one activation
//! per pump, rewake while more remain. The two crates keep separate copies
//! (each owns its own platform's menu vocabulary) but must not diverge in
//! behavior.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use frust_shell_desktop::config::{MenuItemSpec, MenuRole, MenuSpec};
use winit::window::Window;

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

    /// The ids that may be reported as activations — i.e. exactly the ids an
    /// activation must carry to reach app code.
    ///
    /// Published to [`MenuBridge::set_known_ids`] when the menu is installed,
    /// so the filter runs in the platform handler rather than per frame.
    pub(crate) fn activation_ids(&self) -> &HashSet<String> {
        &self.activation_ids
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
    window: &Window,
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
    window: &Window,
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

/// The most queued-but-undelivered activations kept before the oldest are
/// dropped.
///
/// A bound is needed because the queue drains a frame at a time (one activation
/// per frame, see the module docs) while nothing stops the platform from
/// enqueueing faster — a held accelerator repeating into a window whose frames
/// are stalled (minimized, a modal loop, a long-running frame) would otherwise
/// grow it without limit. 64 is far beyond any real burst.
const MAX_PENDING: usize = 64;

/// The redraw wake a menu activation triggers, behind a trait so the queue is
/// testable without a live event loop (a `winit::Window` cannot be constructed
/// without one).
pub(crate) trait MenuWaker: Send + Sync {
    /// Ask the shell for a frame, so `pump` runs and takes from the queue.
    fn wake(&self);
}

impl MenuWaker for Window {
    fn wake(&self) {
        self.request_redraw();
    }
}

/// The queue between `muda`'s menu-event handler and the shell's per-frame
/// pump (see the module docs).
#[derive(Debug, Default)]
pub(crate) struct MenuBridge {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// The ids of the app items actually present in the installed menu (the
    /// plan's [`MenuPlan::activation_ids`]). An activation whose id is not here
    /// is dropped rather than forwarded: muda assigns its own ids to the
    /// predefined items, and app code should never see one.
    known_ids: HashSet<String>,
    pending: VecDeque<String>,
    waker: Option<Arc<dyn MenuWaker>>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner")
            .field("known_ids", &self.known_ids)
            .field("pending", &self.pending)
            .field("waker", &self.waker.is_some())
            .finish()
    }
}

impl MenuBridge {
    /// An empty bridge that accepts no id and wakes nothing.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Declare which ids the installed menu can report.
    pub(crate) fn set_known_ids(&self, ids: HashSet<String>) {
        self.lock().known_ids = ids;
    }

    /// Install the redraw target an activation wakes.
    pub(crate) fn set_waker(&self, waker: Arc<dyn MenuWaker>) {
        self.lock().waker = Some(waker);
    }

    /// Queue an activation, returning whether it was accepted (i.e. whether the
    /// id belongs to the installed menu).
    ///
    /// Called from muda's handler — which may run on any thread (see the module
    /// docs) — so it does nothing but queue and wake. The wake happens after
    /// the lock is released, so a waker that re-enters this bridge cannot
    /// deadlock.
    pub(crate) fn submit(&self, id: &str) -> bool {
        let waker = {
            let mut inner = self.lock();
            if !inner.known_ids.contains(id) {
                log::trace!("frust-shell-windows: ignoring non-app menu event {id:?}");
                return false;
            }
            if inner.pending.len() >= MAX_PENDING {
                let dropped = inner.pending.pop_front();
                log::warn!(
                    "frust-shell-windows: menu-event queue full ({MAX_PENDING}); dropped \
                     {dropped:?} — no frame is draining it"
                );
            }
            inner.pending.push_back(id.to_string());
            inner.waker.clone()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        true
    }

    /// Take the oldest queued activation, if any, asking for another frame when
    /// the queue is not empty afterwards.
    ///
    /// One per call on purpose — see the module docs' single-slot rationale.
    /// The rewake is what keeps the remaining activations moving: `pump` runs
    /// only from a redraw, so a queue left non-empty without one would stall
    /// until the next unrelated frame.
    ///
    /// Like [`submit`](Self::submit), the wake happens after the lock is
    /// released.
    pub(crate) fn take_next(&self) -> Option<String> {
        let (id, waker) = {
            let mut inner = self.lock();
            let id = inner.pending.pop_front()?;
            let waker = if inner.pending.is_empty() {
                None
            } else {
                inner.waker.clone()
            };
            (id, waker)
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        Some(id)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // Poison-tolerant: `submit` runs from the platform's own menu dispatch,
        // and the state behind the lock is a queue plus two slots — nothing an
        // unwind elsewhere could leave half-updated in a way a later frame
        // could misread.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The process-global bridge.
///
/// Global because muda's handler slot is global and can be set exactly once per
/// process (its own `OnceCell`), so the handler cannot capture an extension
/// instance that might be rebuilt.
pub(crate) fn bridge() -> &'static MenuBridge {
    static BRIDGE: OnceLock<MenuBridge> = OnceLock::new();
    BRIDGE.get_or_init(MenuBridge::new)
}

/// Route muda's activations into this crate's queue, exactly once per process.
///
/// muda's own slot is a `OnceCell`, so a second registration anywhere in the
/// process is silently ignored; the `Once` here keeps *this* crate from being
/// that second registration when a shell is rebuilt.
///
/// Both routes into the menu land in this handler: a click on the menu bar, and
/// a keystroke the message hook turned into a `WM_COMMAND` via
/// `TranslateAcceleratorW`.
#[cfg(target_os = "windows")]
pub(crate) fn install_event_handler() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        muda::MenuEvent::set_event_handler(Some(|event: muda::MenuEvent| {
            bridge().submit(event.id().as_ref());
        }));
    });
}

// Inert off Windows: muda is not compiled in, so there is nothing to route.
#[cfg(not(target_os = "windows"))]
pub(crate) fn install_event_handler() {}

/// Hand one queued activation to `frust_reactive`, once per frame.
///
/// Platform-free: by this point the activation is a plain id the handler
/// already filtered against the plan (see the module docs), so the same body
/// serves every target — off Windows nothing ever fills the queue.
pub(crate) fn pump_menu_events() {
    if let Some(id) = bridge().take_next() {
        frust_reactive::push_menu_event(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, label: &str) -> MenuItemSpec {
        MenuItemSpec::item(id, label)
    }

    /// Whether an activation carrying `id` would reach app code — the filter
    /// the bridge applies, asked of the plan that publishes the ids.
    fn activates(plan: &MenuPlan, id: &str) -> bool {
        plan.activation_ids().contains(id)
    }

    fn ids(items: &[&str]) -> HashSet<String> {
        items.iter().map(|id| (*id).to_string()).collect()
    }

    /// A recording waker: the redraw request a live shell would make.
    #[derive(Debug, Default)]
    struct FakeWaker {
        wakes: Mutex<usize>,
    }

    impl FakeWaker {
        fn wakes(&self) -> usize {
            *self.wakes.lock().expect("test mutex")
        }
    }

    impl MenuWaker for FakeWaker {
        fn wake(&self) {
            *self.wakes.lock().expect("test mutex") += 1;
        }
    }

    /// Every queued activation, taken one per call the way successive frames
    /// would take them.
    fn drained(bridge: &MenuBridge) -> Vec<String> {
        std::iter::from_fn(|| bridge.take_next()).collect()
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

        assert!(activates(&plan, "file.open"));
        // Nested two levels down — the bridge must still accept it.
        assert!(activates(&plan, "edit.undo"));
        // muda assigns predefined items ids of their own; none of them is the
        // app's, so none is ever forwarded.
        assert!(!activates(&plan, "File"));
        assert!(!activates(&plan, "1000"));
        assert!(!activates(&plan, ""));
    }

    // --- the activation queue ---

    #[test]
    fn a_known_activation_queues_and_wakes_the_shell() {
        // The whole point of the push bridge: an activation arriving while the
        // dirty-driven loop is idle asks for the frame that will deliver it.
        let bridge = MenuBridge::new();
        let waker = Arc::new(FakeWaker::default());
        bridge.set_known_ids(ids(&["file.open"]));
        bridge.set_waker(waker.clone());

        assert!(bridge.submit("file.open"));
        assert_eq!(waker.wakes(), 1);
        assert_eq!(drained(&bridge), vec!["file.open".to_string()]);
        // Taking empties the queue: a later frame must not re-report an
        // activation it already pushed.
        assert!(drained(&bridge).is_empty());
    }

    #[test]
    fn an_unknown_activation_is_dropped_without_a_wake() {
        // muda assigns its own ids to the predefined items this shell builds;
        // none of them may reach app code, nor cost a frame.
        let bridge = MenuBridge::new();
        let waker = Arc::new(FakeWaker::default());
        bridge.set_known_ids(ids(&["file.open"]));
        bridge.set_waker(waker.clone());

        assert!(!bridge.submit("1000"));
        assert_eq!(waker.wakes(), 0);
        assert!(drained(&bridge).is_empty());
    }

    #[test]
    fn activations_are_taken_in_order() {
        let bridge = MenuBridge::new();
        bridge.set_known_ids(ids(&["a", "b"]));
        bridge.submit("a");
        bridge.submit("b");
        bridge.submit("a");
        assert_eq!(
            drained(&bridge),
            vec!["a".to_string(), "b".to_string(), "a".to_string()]
        );
    }

    #[test]
    fn a_frame_takes_one_activation_and_asks_for_another_frame() {
        // The single-slot push contract: each `pump` hands `push_menu_event`
        // exactly one activation, and the queue's remainder is carried by a
        // further redraw rather than by a batch the frame's one tracked read
        // would coalesce.
        let bridge = MenuBridge::new();
        let waker = Arc::new(FakeWaker::default());
        bridge.set_known_ids(ids(&["a", "b", "c"]));
        bridge.set_waker(waker.clone());

        bridge.submit("a");
        bridge.submit("b");
        bridge.submit("c");
        let after_submits = waker.wakes();

        assert_eq!(bridge.take_next().as_deref(), Some("a"));
        assert_eq!(
            waker.wakes(),
            after_submits + 1,
            "two activations still queued must wake the shell again"
        );
        assert_eq!(bridge.take_next().as_deref(), Some("b"));
        assert_eq!(waker.wakes(), after_submits + 2);

        // The last one empties the queue: nothing left to carry, no frame to
        // ask for.
        assert_eq!(bridge.take_next().as_deref(), Some("c"));
        assert_eq!(waker.wakes(), after_submits + 2);
    }

    #[test]
    fn taking_from_an_empty_queue_neither_yields_nor_wakes() {
        // Every idle frame calls `pump`; none of them may request another.
        let bridge = MenuBridge::new();
        let waker = Arc::new(FakeWaker::default());
        bridge.set_waker(waker.clone());

        assert_eq!(bridge.take_next(), None);
        assert_eq!(waker.wakes(), 0);
    }

    #[test]
    fn a_bridge_with_no_waker_still_queues() {
        // The handler is installed before the window exists in principle;
        // losing the activation would be worse than losing the wake.
        let bridge = MenuBridge::new();
        bridge.set_known_ids(ids(&["a"]));
        assert!(bridge.submit("a"));
        assert_eq!(drained(&bridge), vec!["a".to_string()]);
    }

    #[test]
    fn the_queue_is_bounded_and_drops_the_oldest() {
        let bridge = MenuBridge::new();
        bridge.set_known_ids(ids(&["a", "newest"]));
        for _ in 0..MAX_PENDING {
            bridge.submit("a");
        }
        bridge.submit("newest");

        let out = drained(&bridge);
        assert_eq!(out.len(), MAX_PENDING);
        assert_eq!(out.last().map(String::as_str), Some("newest"));
    }

    #[test]
    fn the_process_global_bridge_is_one_instance() {
        assert!(std::ptr::eq(bridge(), bridge()));
    }

    #[test]
    fn the_plan_publishes_exactly_the_ids_the_bridge_filters_on() {
        // The two halves of the filter: the plan derives the id set once, and
        // the bridge is what actually applies it per activation.
        let plan = MenuPlan::from_spec(Some(
            &MenuSpec::new().with_item(MenuItemSpec::submenu(
                "File",
                MenuSpec::new()
                    .with_item(item("file.open", "Open…"))
                    .with_item(MenuItemSpec::role(MenuRole::Quit)),
            )),
        ));
        let bridge = MenuBridge::new();
        bridge.set_known_ids(plan.activation_ids().clone());

        assert!(bridge.submit("file.open"));
        assert!(!bridge.submit("1000"));
        assert_eq!(drained(&bridge), vec!["file.open".to_string()]);
    }
}
