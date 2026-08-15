//! The native menu bar: a `muda` `NSMenu` tree built from
//! [`DesktopConfig`](frust_shell_desktop::DesktopConfig), and the queue that
//! carries an activation from AppKit's menu dispatch into the shell's per-frame
//! pump.
//!
//! # Shape of the menu
//!
//! macOS gives every app an *application menu* — the bold, app-named first
//! menu carrying About/Hide/Quit — that no app declares item by item. This
//! module always builds it (from `DesktopConfig::app_name`), then appends the
//! app's own [`MenuSpec`] after it, so an app that declares no menu at all
//! still gets a conventional menu bar with a working ⌘Q, and an app that
//! declares one never has to hand-roll the standard items.
//!
//! Everything in the application menu is a `muda` *predefined* item, which on
//! macOS maps to the platform's own selector (`terminate:`, `hide:`,
//! `orderFrontStandardAboutPanel:`). Those are performed by AppKit itself and
//! report no activation — which is exactly why they keep working while this
//! shell's window is hidden and no frame is being produced (see the pump note
//! below).
//!
//! # Push, not poll: why activations do not go through `MenuEvent::receiver`
//!
//! `muda` publishes activations two ways: a process-global channel
//! (`MenuEvent::receiver()`) and a handler callback
//! (`MenuEvent::set_event_handler`). Draining the channel from
//! [`DesktopExtensions::pump`](frust_shell_desktop::extensions::DesktopExtensions::pump)
//! alone would lose the wake: the shared desktop core is dirty-driven
//! (`ControlFlow::Wait`) and only calls `pump` from a redraw, so an activation
//! arriving while the app is idle would sit in the channel until some unrelated
//! event happened to produce a frame. So this module installs the handler
//! instead, queues the activation, and *requests a redraw* — the loop wakes,
//! `pump` drains the queue at the top of that frame, and the resulting
//! `frust_reactive::push_menu_event` is visible to the very same frame's
//! rebuild.
//!
//! The queue is also what keeps the reactive push on the UI thread: `muda`'s
//! handler is `Send + Sync` and its threading is the platform's business, while
//! `push_menu_event` documents a UI-thread-only contract.
//!
//! **Residual:** while the window is hidden (see [`crate::lifecycle`]) no
//! frames are produced, so an *app* item activated in that state stays queued
//! until the window is shown again. The standard items are unaffected — AppKit
//! performs those itself.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use frust_shell_desktop::config::{MenuItemSpec, MenuSpec};
use winit::window::Window;

/// The most queued-but-undelivered activations kept before the oldest are
/// dropped.
///
/// A bound is needed because the queue drains from a frame, and a hidden window
/// produces none (see the module docs) — without it, an app left hidden with a
/// key-equivalent held down would grow the queue for as long as it stayed
/// hidden. 64 is far beyond any real burst between two frames.
const MAX_PENDING: usize = 64;

/// The redraw wake a menu activation triggers, behind a trait so the queue is
/// testable without a live event loop (a `winit::Window` cannot be constructed
/// without one).
pub(crate) trait MenuWaker: Send + Sync {
    /// Ask the shell for a frame, so `pump` runs and drains the queue.
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
    /// The ids of the app items actually present in the installed menu.
    /// An activation whose id is not here is dropped rather than forwarded:
    /// `muda` assigns its own ids to items this shell did not name, and app
    /// code should never see one.
    known_ids: BTreeSet<String>,
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

    /// Declare which ids the installed menu can report (see
    /// [`collect_app_item_ids`]).
    pub(crate) fn set_known_ids(&self, ids: BTreeSet<String>) {
        self.lock().known_ids = ids;
    }

    /// Install the redraw target an activation wakes.
    pub(crate) fn set_waker(&self, waker: Arc<dyn MenuWaker>) {
        self.lock().waker = Some(waker);
    }

    /// Queue an activation, returning whether it was accepted (i.e. whether
    /// the id belongs to the installed menu).
    ///
    /// Called from `muda`'s handler; the wake happens after the lock is
    /// released, so a waker that re-enters this bridge cannot deadlock.
    pub(crate) fn submit(&self, id: &str) -> bool {
        let waker = {
            let mut inner = self.lock();
            if !inner.known_ids.contains(id) {
                log::debug!("frust-shell-macos: ignoring menu event for unknown id {id:?}");
                return false;
            }
            if inner.pending.len() >= MAX_PENDING {
                let dropped = inner.pending.pop_front();
                log::warn!(
                    "frust-shell-macos: menu-event queue full ({MAX_PENDING}); dropped {dropped:?} \
                     — the window is likely hidden, so no frame is draining it"
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

    /// Move every queued activation into `out`, oldest first.
    pub(crate) fn drain_into(&self, out: &mut Vec<String>) {
        let mut inner = self.lock();
        out.extend(inner.pending.drain(..));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // Poison-tolerant: `submit` runs from AppKit's own menu dispatch,
        // where an unwind would be undefined behavior, and the state behind
        // the lock is a queue plus two slots (see `Lifecycle::lock`).
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The process-global bridge.
///
/// Global because `muda`'s handler slot is global and can be set exactly once
/// per process (its own `OnceCell`), so the handler cannot capture an
/// extension instance that might be rebuilt.
pub(crate) fn bridge() -> &'static MenuBridge {
    static BRIDGE: OnceLock<MenuBridge> = OnceLock::new();
    BRIDGE.get_or_init(MenuBridge::new)
}

/// Every app-item id in `spec`, including nested submenus.
///
/// Role items and separators contribute nothing — they report no activation
/// (`MenuItemSpec::id`).
pub(crate) fn collect_app_item_ids(spec: Option<&MenuSpec>) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    if let Some(spec) = spec {
        collect_into(spec, &mut ids);
    }
    ids
}

fn collect_into(spec: &MenuSpec, ids: &mut BTreeSet<String>) {
    for item in &spec.items {
        match item {
            MenuItemSpec::Item { id, .. } => {
                if !ids.insert(id.clone()) {
                    log::warn!(
                        "frust-shell-macos: duplicate menu item id {id:?} — an activation of \
                         either item reports the same id"
                    );
                }
            }
            MenuItemSpec::Submenu { menu, .. } => collect_into(menu, ids),
            MenuItemSpec::Role { .. } | MenuItemSpec::Separator => {}
        }
    }
}

/// The label for the standard About item — macOS spells it "About <App>".
pub(crate) fn about_label(app_name: &str) -> String {
    format!("About {app_name}")
}

/// The label for the standard Hide item ("Hide <App>", ⌘H).
pub(crate) fn hide_label(app_name: &str) -> String {
    format!("Hide {app_name}")
}

/// The label for the standard Quit item ("Quit <App>", ⌘Q).
pub(crate) fn quit_label(app_name: &str) -> String {
    format!("Quit {app_name}")
}

#[cfg(target_os = "macos")]
pub(crate) use platform::{install_event_handler, install_menu};

#[cfg(target_os = "macos")]
mod platform {
    //! The `muda` half: building the `NSMenu` tree and installing it on
    //! `NSApp`. Off macOS none of this compiles (the crate declares `muda`
    //! only in a `cfg(target_os = "macos")` dependency table) and the shell's
    //! menu hooks are inert.

    use std::sync::Once;

    use frust_shell_desktop::config::{MenuItemSpec, MenuRole, MenuSpec};
    use muda::accelerator::Accelerator;
    use muda::{IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

    use super::{about_label, bridge, hide_label, quit_label};

    /// Route `muda`'s activations into this crate's queue, exactly once per
    /// process.
    ///
    /// `muda`'s own slot is a `OnceCell`, so a second registration anywhere in
    /// the process is silently ignored; the `Once` here keeps *this* crate
    /// from being that second registration when a shell is rebuilt.
    pub(crate) fn install_event_handler() {
        static INSTALLED: Once = Once::new();
        INSTALLED.call_once(|| {
            MenuEvent::set_event_handler(Some(|event: MenuEvent| {
                bridge().submit(event.id.as_ref());
            }));
        });
    }

    /// Build the menu bar and make it `NSApp`'s main menu, returning the menu
    /// to retain (dropping it would tear the native menu down).
    ///
    /// This *replaces* the placeholder menu bar `winit` installs while
    /// launching (`EventLoopBuilderExtMacOS::with_default_menu`, on by
    /// default). Suppressing that one at the builder is deliberately not done:
    /// if the build below fails, winit's default — which carries a working ⌘Q
    /// of its own — is what the app is left with, rather than no menu bar at
    /// all.
    pub(crate) fn install_menu(app_name: &str, spec: Option<&MenuSpec>) -> Option<Menu> {
        let menu = match build_menu(app_name, spec) {
            Ok(menu) => menu,
            Err(err) => {
                log::warn!("frust-shell-macos: failed to build the native menu bar: {err}");
                return None;
            }
        };
        menu.init_for_nsapp();
        Some(menu)
    }

    /// The application menu, then the app's own items (see the module docs).
    fn build_menu(app_name: &str, spec: Option<&MenuSpec>) -> muda::Result<Menu> {
        let menu = Menu::new();
        menu.append(&app_submenu(app_name)?)?;
        if let Some(spec) = spec {
            for item in &spec.items {
                if !matches!(item, MenuItemSpec::Submenu { .. }) {
                    log::debug!(
                        "frust-shell-macos: a macOS menu bar shows only submenus at its top \
                         level; the declared item is installed but may not be reachable"
                    );
                }
                append(&menu, item)?;
            }
        }
        Ok(menu)
    }

    /// The bold, app-named first menu every macOS app carries.
    fn app_submenu(app_name: &str) -> muda::Result<Submenu> {
        // Bound before the array so each label outlives the `&str` borrowed
        // into it (`Option<&str>`, not `Option<&String>` — no deref coercion
        // reaches inside an `Option`).
        let about = about_label(app_name);
        let hide = hide_label(app_name);
        let quit = quit_label(app_name);

        let app_menu = Submenu::new(app_name, true);
        app_menu.append_items(&[
            &PredefinedMenuItem::about(Some(about.as_str()), None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(Some(hide.as_str())),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::quit(Some(quit.as_str())),
        ])?;
        Ok(app_menu)
    }

    /// Translate one spec item and append it to `container` (a `Menu` or a
    /// `Submenu` — both implement `append`).
    fn append(container: &dyn Container, item: &MenuItemSpec) -> muda::Result<()> {
        match item {
            MenuItemSpec::Item {
                id,
                label,
                accelerator,
                enabled,
            } => {
                let accelerator = accelerator.as_deref().and_then(parse_accelerator);
                container.append_item(&MenuItem::with_id(
                    id.as_str(),
                    label,
                    *enabled,
                    accelerator,
                ))
            }
            MenuItemSpec::Role { role, label } => {
                container.append_item(&predefined(*role, label.as_deref()))
            }
            MenuItemSpec::Separator => container.append_item(&PredefinedMenuItem::separator()),
            MenuItemSpec::Submenu { label, menu } => {
                let submenu = Submenu::new(label, true);
                for child in &menu.items {
                    append(&submenu, child)?;
                }
                container.append_item(&submenu)
            }
        }
    }

    /// One standard role as `muda`'s platform-performed item. Every label
    /// override is honored; `None` keeps the platform's own (localized) text.
    fn predefined(role: MenuRole, label: Option<&str>) -> PredefinedMenuItem {
        match role {
            MenuRole::About => PredefinedMenuItem::about(label, None),
            MenuRole::Hide => PredefinedMenuItem::hide(label),
            MenuRole::HideOthers => PredefinedMenuItem::hide_others(label),
            MenuRole::ShowAll => PredefinedMenuItem::show_all(label),
            MenuRole::Minimize => PredefinedMenuItem::minimize(label),
            MenuRole::CloseWindow => PredefinedMenuItem::close_window(label),
            MenuRole::Quit => PredefinedMenuItem::quit(label),
        }
    }

    /// Parse the cross-platform accelerator shorthand, reporting an
    /// unparseable one and installing the item without a shortcut rather than
    /// failing the whole menu.
    ///
    /// This is the one place the shorthand is validated (see
    /// `MenuItemSpec::Item::accelerator`'s docs — the vocabulary crate binds no
    /// menu library and deliberately does not validate it twice).
    fn parse_accelerator(accelerator: &str) -> Option<Accelerator> {
        match accelerator.parse::<Accelerator>() {
            Ok(parsed) => Some(parsed),
            Err(err) => {
                log::warn!(
                    "frust-shell-macos: ignoring unparseable menu accelerator \
                     {accelerator:?}: {err}"
                );
                None
            }
        }
    }

    /// The append seam shared by the menu bar and a submenu, so
    /// [`append`] can recurse without knowing which it is holding.
    trait Container {
        fn append_item(&self, item: &dyn IsMenuItem) -> muda::Result<()>;
    }

    impl Container for Menu {
        fn append_item(&self, item: &dyn IsMenuItem) -> muda::Result<()> {
            self.append(item)
        }
    }

    impl Container for Submenu {
        fn append_item(&self, item: &dyn IsMenuItem) -> muda::Result<()> {
            self.append(item)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use frust_shell_desktop::config::{MenuItemSpec, MenuRole};

    use super::*;

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

    fn ids(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|id| (*id).to_string()).collect()
    }

    fn drained(bridge: &MenuBridge) -> Vec<String> {
        let mut out = Vec::new();
        bridge.drain_into(&mut out);
        out
    }

    // --- spec → id set ---

    #[test]
    fn only_app_items_contribute_an_id_and_submenus_are_walked() {
        let file = MenuSpec::new()
            .with_item(MenuItemSpec::item("file.open", "Open…").with_accelerator("CmdOrCtrl+O"))
            .with_item(MenuItemSpec::separator())
            .with_item(MenuItemSpec::role(MenuRole::CloseWindow))
            .with_item(MenuItemSpec::submenu(
                "Recent",
                MenuSpec::new().with_item(MenuItemSpec::item("file.recent.clear", "Clear")),
            ));
        let bar = MenuSpec::new()
            .with_item(MenuItemSpec::submenu("File", file))
            .with_item(MenuItemSpec::submenu(
                "Edit",
                MenuSpec::new().with_item(MenuItemSpec::item("edit.find", "Find").disabled()),
            ));

        assert_eq!(
            collect_app_item_ids(Some(&bar)),
            ids(&["file.open", "file.recent.clear", "edit.find"])
        );
    }

    #[test]
    fn a_missing_or_empty_spec_declares_no_ids() {
        assert!(collect_app_item_ids(None).is_empty());
        assert!(collect_app_item_ids(Some(&MenuSpec::new())).is_empty());
        // A menu of nothing but platform-performed items reports nothing
        // either — those never reach `push_menu_event`.
        let roles = MenuSpec::new()
            .with_item(MenuItemSpec::role(MenuRole::Quit))
            .with_item(MenuItemSpec::separator());
        assert!(collect_app_item_ids(Some(&roles)).is_empty());
    }

    #[test]
    fn a_duplicate_id_collapses_into_one_entry() {
        let spec = MenuSpec::new()
            .with_item(MenuItemSpec::item("dup", "First"))
            .with_item(MenuItemSpec::item("dup", "Second"));
        assert_eq!(collect_app_item_ids(Some(&spec)), ids(&["dup"]));
    }

    // --- standard labels ---

    #[test]
    fn the_standard_labels_follow_the_macos_wording() {
        assert_eq!(about_label("Huddle"), "About Huddle");
        assert_eq!(hide_label("Huddle"), "Hide Huddle");
        assert_eq!(quit_label("Huddle"), "Quit Huddle");
    }

    // --- the activation queue ---

    #[test]
    fn a_known_activation_queues_and_wakes_the_shell() {
        let bridge = MenuBridge::new();
        let waker = Arc::new(FakeWaker::default());
        bridge.set_known_ids(ids(&["file.open"]));
        bridge.set_waker(waker.clone());

        assert!(bridge.submit("file.open"));
        assert_eq!(waker.wakes(), 1);
        assert_eq!(drained(&bridge), vec!["file.open".to_string()]);
        // Draining empties the queue: a frame must not re-report an
        // activation it already pushed.
        assert!(drained(&bridge).is_empty());
    }

    #[test]
    fn an_unknown_activation_is_dropped_without_a_wake() {
        // `muda` assigns its own ids to the predefined items this shell
        // builds; none of them may reach app code.
        let bridge = MenuBridge::new();
        let waker = Arc::new(FakeWaker::default());
        bridge.set_known_ids(ids(&["file.open"]));
        bridge.set_waker(waker.clone());

        assert!(!bridge.submit("1"));
        assert_eq!(waker.wakes(), 0);
        assert!(drained(&bridge).is_empty());
    }

    #[test]
    fn activations_drain_in_order() {
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
}
