//! The iOS chrome arm (decision C2): [`IosChromeHost`] re-roots the window
//! from Rust into a `UITabBarController` whose tabs are `UINavigationController`s,
//! each over one [`FrustChromePageController`], and contains the one frust
//! `UIViewController` in the selected page. Zero Swift.
//!
//! # Precondition
//!
//! The chrome must be the first thing that re-roots the window: the window's
//! `rootViewController` at install IS the frust controller. A root that is
//! already a `UITabBarController`/`UINavigationController` is refused
//! ([`ChromeError::Refused`], "window already re-rooted"). No window yet, or a
//! root presenting a modal, answers [`ChromeError::NoHost`], which the slot
//! retries on the next publish.
//!
//! # Threading and ordering
//!
//! The slot calls the host on the frame's (main) thread with its lock held,
//! inside the frust frame. Re-rooting or moving the frust view there would
//! re-enter the frame through `FrustViewController`'s layout and inset
//! callbacks, so `install` only validates the precondition synchronously and
//! every UIKit mutation (install, each diff, dispose) runs on a later
//! main-queue turn, in publish order. The UIKit objects live in a main-thread
//! `thread_local!`; the host itself holds only the live generation, so it is
//! `Send` and const-constructible for the slot's `static`. A selection move is
//! therefore never made inside a tab-bar delegate callback.
//!
//! # The containment-move contract
//!
//! The frust controller's view (the `FrustView`) is wrapped in a plugin-owned
//! host `UIView`; the controller is a child of exactly one page at a time
//! (`addChildViewController`, host view added, `didMoveToParentViewController`)
//! and a selection change moves it (`willMoveToParentViewController(nil)`,
//! host view removed, `removeFromParentViewController`, then the same three
//! steps on the new page). Pages are kept by `TabId`, so a reorder never
//! loses the contained controller.
//!
//! # The FrustViewHost origin-zero invariant
//!
//! Mode B wrappers are siblings of the `FrustView` in its superview, assumed
//! at origin zero and never moved after creation. Install moves the
//! `FrustView` and every sibling (z-order kept) into the host view, with the
//! `FrustView` at origin zero filling it, so later wrappers land in the host
//! view and every move carries them. Dispose re-roots the window to the frust
//! controller and puts the siblings back around the `FrustView`.
//!
//! # Status bar and home indicator
//!
//! `FrustViewController` computes `prefersStatusBarHidden` and
//! `prefersHomeIndicatorAutoHidden`; the containers already forward to the
//! selected page, and a page forwards to the frust controller while it holds
//! it, so `set_system_ui_mode` keeps working under the chrome.
//!
//! # Events
//!
//! One `FrustNativeControlTarget` per install, held in the chrome state, is
//! the tab bar controller's delegate and the target of the back and plain
//! action items (set as each item is built); a menu item's `UIAction` block
//! captures its action and item indices. Each tap is resolved against the
//! spec UIKit shows (`crate::chrome::resolve_tap`, a tab against the
//! app-confirmed `selected`) and handed to `crate::chrome::deliver_event`,
//! never with a [`CHROME`] borrow held (`try_borrow`: an apply in progress
//! has taken the state, and the tap is dropped). A tab tap answers NO, so
//! UIKit never switches tabs itself.
//!
//! The frust UI thread is the main thread on iOS: taps and applies both run
//! there, so a tap always resolves against the spec on screen, and the
//! by-id re-check covers the one main-queue turn an apply trails its
//! publish. That is why iOS needs no spec-revision stamp (the Android arm,
//! whose delivery may hop threads, gets one).
//!
//! # Brightness
//!
//! `ChromeSpec::dark` is pinned with `overrideUserInterfaceStyle` on the tab
//! bar and every navigation bar, not on the tab bar controller: a controller
//! override reaches the contained frust controller, whose trait changes
//! feed the app theme's brightness and would pin it. The bars are restyled
//! on install, for every new page, and on any apply whose spec flips it.
//!
//! # Limitations
//!
//! `native-chrome-single-surface-containment`: one frust surface serves every
//! tab, and the navigation controllers have no stack (no native push
//! animation; `Leading::Back` is a custom item reporting `BackPressed`).

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_core_foundation::CGFloat;
use objc2_foundation::{NSArray, NSData, NSString, ns_string};
use objc2_ui_kit::{
    NSObjectUIAccessibility, UIAction, UIApplication, UIBarButtonItem, UIImage, UIImageView,
    UIMenu, UIMenuElement, UIMenuElementAttributes, UINavigationController,
    UINavigationItemLargeTitleDisplayMode, UIResponder, UISceneActivationState, UITabBarController,
    UIView, UIViewAutoresizing, UIViewContentMode, UIViewController, UIWindow, UIWindowScene,
};

use super::FrustNativeControlTarget;
use super::theme::apply_user_interface_style;
use crate::api::{TabIcon, TabId, TabItem};
use crate::chrome::host::ChromeField;
use crate::chrome::{
    self, ActionItem, ChromeDiff, ChromeError, ChromeGeneration, ChromeHost, ChromeSpec, ChromeTap,
    Leading, MenuItem, TopBarSpec, resolve_tap,
};
use crate::controls::tab_bar::icon_decode_scale;

/// The leading avatar's side, in points.
const AVATAR_POINTS: CGFloat = 32.0;

/// The process-wide chrome host on iOS — see the module doc.
#[derive(Debug, Default)]
pub(crate) struct IosChromeHost {
    /// The installed generation; `None` before install and after dispose.
    live: Option<u64>,
}

impl IosChromeHost {
    /// An idle host (const, for the slot's `static`).
    pub(crate) const fn new() -> Self {
        Self { live: None }
    }
}

impl ChromeHost for IosChromeHost {
    fn install(
        &mut self,
        generation: ChromeGeneration,
        spec: &ChromeSpec,
    ) -> Result<(), ChromeError> {
        let mtm = MainThreadMarker::new().ok_or(ChromeError::NoHost)?;
        check_host(mtm)?;
        let generation = generation.get();
        let spec = spec.clone();
        self.live = Some(generation);
        on_main(move |mtm| install_now(mtm, generation, &spec));
        Ok(())
    }

    fn apply(&mut self, diff: &ChromeDiff<'_>) {
        let Some(generation) = self.live else {
            return;
        };
        let spec = diff.spec.clone();
        let changed = diff.changed.clone();
        on_main(move |mtm| {
            let diff = ChromeDiff {
                spec: &spec,
                changed,
            };
            with_chrome(generation, |chrome| chrome.apply(mtm, &diff));
        });
    }

    fn dispose(&mut self) {
        if let Some(generation) = self.live.take() {
            on_main(move |mtm| {
                if let Some(chrome) = take_chrome(generation) {
                    chrome.dispose(mtm);
                }
            });
        }
    }
}

thread_local! {
    /// The installed chrome, if any. Main thread only.
    static CHROME: RefCell<Option<Chrome>> = const { RefCell::new(None) };
}

/// Take generation `generation`'s chrome out of [`CHROME`] (any when `None`).
fn take_chrome_matching(generation: Option<u64>) -> Option<Chrome> {
    CHROME.with(|cell| {
        let mut slot = cell.borrow_mut();
        match (slot.as_ref(), generation) {
            (Some(chrome), Some(wanted)) if chrome.generation != wanted => None,
            _ => slot.take(),
        }
    })
}

fn take_chrome(generation: u64) -> Option<Chrome> {
    take_chrome_matching(Some(generation))
}

/// Run `f` on `generation`'s chrome, outside the `RefCell` borrow so a UIKit
/// callback reaching [`CHROME`] meanwhile finds it empty rather than panics.
fn with_chrome(generation: u64, f: impl FnOnce(&mut Chrome)) {
    if let Some(mut chrome) = take_chrome(generation) {
        f(&mut chrome);
        CHROME.with(|cell| *cell.borrow_mut() = Some(chrome));
    }
}

/// Run `f` on the main thread, on a later main-queue turn, under
/// `catch_unwind` (no unwind may cross libdispatch's frames).
fn on_main(f: impl FnOnce(MainThreadMarker) + Send + 'static) {
    DispatchQueue::main().exec_async(move || {
        // SAFETY: a block submitted to `dispatch_get_main_queue()` always
        // runs on the process's main thread (`present::apple_host::on_main`
        // makes the same claim).
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        if catch_unwind(AssertUnwindSafe(|| f(mtm))).is_err() {
            log::error!("frust-native-widgets: a native chrome step panicked on the main queue");
        }
    });
}

/// The window of a foreground-active window scene (key window first), else
/// any window scene's — `present::apple_host`'s scene walk.
fn anchor_window(mtm: MainThreadMarker) -> Option<Retained<UIWindow>> {
    let mut fallback = None;
    for scene in UIApplication::sharedApplication(mtm)
        .connectedScenes()
        .iter()
    {
        let Some(window_scene) = scene.downcast_ref::<UIWindowScene>() else {
            continue;
        };
        let Some(window) = window_scene
            .keyWindow()
            .or_else(|| window_scene.windows().firstObject())
        else {
            continue;
        };
        if scene.activationState() == UISceneActivationState::ForegroundActive {
            return Some(window);
        }
        fallback = fallback.or(Some(window));
    }
    fallback
}

fn is_container(controller: &UIViewController) -> bool {
    controller.downcast_ref::<UITabBarController>().is_some()
        || controller
            .downcast_ref::<UINavigationController>()
            .is_some()
}

/// The install precondition (module doc), checked on the frame thread.
fn check_host(mtm: MainThreadMarker) -> Result<(), ChromeError> {
    let window = anchor_window(mtm).ok_or(ChromeError::NoHost)?;
    let root = window.rootViewController().ok_or(ChromeError::NoHost)?;
    // Our own chrome still rooted: its dispose is queued ahead of this install.
    let ours = CHROME.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_some_and(|chrome| std::ptr::eq(&*root, &**chrome.tabs))
    });
    if ours {
        return Ok(());
    }
    if is_container(&root) {
        return Err(ChromeError::Refused("window already re-rooted".into()));
    }
    if root.presentedViewController().is_some() {
        return Err(ChromeError::NoHost);
    }
    Ok(())
}

/// The queued half of [`IosChromeHost::install`].
fn install_now(mtm: MainThreadMarker, generation: u64, spec: &ChromeSpec) {
    if let Some(stale) = take_chrome_matching(None) {
        stale.dispose(mtm);
    }
    let root = anchor_window(mtm).and_then(|window| Some((window.rootViewController()?, window)));
    let Some((frust, window)) = root else {
        log::warn!("frust-native-widgets: native chrome found no window to re-root");
        return;
    };
    if is_container(&frust) || frust.presentedViewController().is_some() {
        log::warn!("frust-native-widgets: native chrome skipped; the window root changed");
        return;
    }
    let chrome = Chrome::install(mtm, generation, &window, frust, spec);
    CHROME.with(|cell| *cell.borrow_mut() = Some(chrome));
}

/// One tab: its navigation controller and the page under it.
struct Page {
    /// `None` for the one implicit page of a tab-less spec.
    id: Option<TabId>,
    nav: Retained<UINavigationController>,
    page: Retained<FrustChromePageController>,
}

/// Everything the installed chrome owns.
struct Chrome {
    generation: u64,
    window: Weak<UIWindow>,
    frust: Retained<UIViewController>,
    /// The plugin-owned view wrapping the `FrustView` and its siblings.
    host: Retained<UIView>,
    tabs: Retained<UITabBarController>,
    pages: Vec<Page>,
    /// The spec UIKit shows: what taps resolve against.
    spec: ChromeSpec,
    /// The delegate and bar-item target; both references to it are weak.
    target: Retained<FrustNativeControlTarget>,
}

impl Chrome {
    fn install(
        mtm: MainThreadMarker,
        generation: u64,
        window: &Retained<UIWindow>,
        frust: Retained<UIViewController>,
        spec: &ChromeSpec,
    ) -> Self {
        let frust_view = frust.view();
        let stack: Vec<Retained<UIView>> = frust_view
            .as_ref()
            .and_then(|view| view.superview())
            .map(|superview| superview.subviews().iter().collect())
            .unwrap_or_default();
        let host = UIView::new(mtm);
        let flexible = UIViewAutoresizing::FlexibleWidth | UIViewAutoresizing::FlexibleHeight;
        host.setAutoresizingMask(flexible);
        if let Some(view) = &frust_view {
            host.setFrame(view.frame());
        }
        let mut chrome = Self {
            generation,
            window: Weak::from_retained(window),
            frust,
            host,
            tabs: UITabBarController::new(mtm),
            pages: Vec::new(),
            spec: spec.clone(),
            target: FrustNativeControlTarget::chrome_target(mtm),
        };
        chrome.target.attach_tab_bar_controller(&chrome.tabs);
        chrome.sync_pages(mtm, spec);
        apply_user_interface_style(&chrome.tabs.tabBar(), spec.dark);
        // The root swap takes the `FrustView` out of the window; its siblings
        // stay until moved, in their old order, into the host view.
        window.setRootViewController(Some(&chrome.tabs));
        for view in &stack {
            chrome.host.addSubview(view);
        }
        if let Some(view) = frust_view {
            view.setFrame(chrome.host.bounds());
            view.setAutoresizingMask(flexible);
        }
        chrome.select(spec);
        chrome
    }

    /// `ChromeField::Scroll` is ignored here: the iOS bars do not react to the
    /// body's scroll (the scaffold draws the edge fade instead).
    fn apply(&mut self, mtm: MainThreadMarker, diff: &ChromeDiff<'_>) {
        let spec = diff.spec;
        let restyle = self.spec.dark != spec.dark;
        self.spec = spec.clone();
        if diff.contains(ChromeField::Tabs) {
            self.sync_pages(mtm, spec);
        }
        if diff.contains(ChromeField::Tabs) || diff.contains(ChromeField::Selected) {
            self.select(spec);
        }
        for page in &self.pages {
            let top = self.spec.top.as_ref();
            apply_top(mtm, page, top, &self.target, |field| diff.contains(field));
        }
        if restyle {
            apply_user_interface_style(&self.tabs.tabBar(), spec.dark);
            for page in &self.pages {
                apply_user_interface_style(&page.nav.navigationBar(), spec.dark);
            }
        }
    }

    /// Rebuild the view-controller array for `spec.tabs`, keeping pages by id.
    fn sync_pages(&mut self, mtm: MainThreadMarker, spec: &ChromeSpec) {
        let wanted: Vec<Option<&TabItem>> = if spec.tabs.is_empty() {
            vec![None]
        } else {
            spec.tabs.iter().map(Some).collect()
        };
        let mut old = std::mem::take(&mut self.pages);
        for tab in wanted {
            let id = tab.map(|tab| tab.id().clone());
            let page = match old.iter().position(|page| page.id == id) {
                Some(index) => old.remove(index),
                None => {
                    let page = Page::new(mtm, id);
                    apply_top(mtm, &page, self.spec.top.as_ref(), &self.target, |_| true);
                    apply_user_interface_style(&page.nav.navigationBar(), self.spec.dark);
                    page
                }
            };
            if let Some(tab) = tab {
                page.set_tab_item(tab);
            }
            self.pages.push(page);
        }
        for page in old {
            page.page.release();
        }
        let navs: Vec<Retained<UIViewController>> = self
            .pages
            .iter()
            .map(|page| Retained::into_super(page.nav.clone()))
            .collect();
        self.tabs
            .setViewControllers(Some(&NSArray::from_retained_slice(&navs)));
        self.tabs.tabBar().setHidden(spec.tabs.is_empty());
    }

    /// Select `spec.selected` (an unknown id keeps the current tab) and move
    /// the frust controller into that page.
    fn select(&mut self, spec: &ChromeSpec) {
        let current = self.tabs.selectedIndex();
        let index = spec
            .selected
            .as_ref()
            .and_then(|id| spec.tabs.iter().position(|tab| tab.id() == id))
            .unwrap_or(current);
        let index = if index < self.pages.len() { index } else { 0 };
        let Some(target) = self.pages.get(index) else {
            return;
        };
        self.tabs.setSelectedIndex(index);
        if !target.page.holds() {
            for page in &self.pages {
                page.page.release();
            }
            target.page.hold(&self.frust, &self.host);
        }
        self.tabs.setNeedsStatusBarAppearanceUpdate();
        self.tabs.setNeedsUpdateOfHomeIndicatorAutoHidden();
    }

    /// Re-root the window back to the frust controller and release the rest.
    fn dispose(self, _mtm: MainThreadMarker) {
        self.target.detach_tab_bar_controller(&self.tabs);
        let stack: Vec<Retained<UIView>> = self.host.subviews().iter().collect();
        for page in &self.pages {
            page.page.release();
        }
        let Some(window) = self.window.load() else {
            return;
        };
        let rooted = window
            .rootViewController()
            .is_some_and(|root| std::ptr::eq(&*root, &**self.tabs));
        if !rooted {
            log::warn!("frust-native-widgets: the window was re-rooted under the native chrome");
            return;
        }
        window.setRootViewController(Some(&self.frust));
        let Some(frust_view) = self.frust.view() else {
            return;
        };
        if let Some(superview) = frust_view.superview() {
            restore_siblings(&stack, &frust_view, &superview);
        }
    }
}

/// Put `stack` (the host view's subviews, bottom to top) back around
/// `frust_view` in `superview`, keeping their order.
fn restore_siblings(stack: &[Retained<UIView>], frust_view: &UIView, superview: &UIView) {
    let split = stack
        .iter()
        .position(|view| std::ptr::eq(&**view, frust_view));
    let Some(split) = split else {
        stack.iter().for_each(|view| superview.addSubview(view));
        return;
    };
    for view in &stack[..split] {
        superview.insertSubview_belowSubview(view, frust_view);
    }
    let mut anchor: &UIView = frust_view;
    for view in &stack[split + 1..] {
        superview.insertSubview_aboveSubview(view, anchor);
        anchor = view;
    }
}

impl Page {
    fn new(mtm: MainThreadMarker, id: Option<TabId>) -> Self {
        let page = FrustChromePageController::new(mtm);
        let nav = UINavigationController::initWithRootViewController(
            UINavigationController::alloc(mtm),
            &page,
        );
        Self { id, nav, page }
    }

    /// Mirror `tab` onto the navigation controller's `tabBarItem`.
    fn set_tab_item(&self, tab: &TabItem) {
        let Some(item) = self.nav.tabBarItem() else {
            return;
        };
        item.setTitle(Some(&NSString::from_str(tab.title())));
        item.setImage(ui_image(tab.icon()).as_deref());
        let selected = tab.selected_icon_ref().and_then(ui_image);
        item.setSelectedImage(selected.as_deref());
        let badge = tab.badge_text().map(NSString::from_str);
        item.setBadgeValue(badge.as_deref());
        item.setEnabled(tab.is_enabled());
    }
}

/// Mirror the top bar's `wants` fields onto `page`'s navigation item, the
/// rebuilt bar items wired to `target`.
fn apply_top(
    mtm: MainThreadMarker,
    page: &Page,
    top: Option<&TopBarSpec>,
    target: &FrustNativeControlTarget,
    wants: impl Fn(ChromeField) -> bool,
) {
    let empty = TopBarSpec::default();
    let bar = top.unwrap_or(&empty);
    let item = page.page.navigationItem();
    if wants(ChromeField::TopBar) {
        page.nav.setNavigationBarHidden(top.is_none());
    }
    if wants(ChromeField::Title) {
        item.setTitle(Some(&NSString::from_str(&bar.title)));
    }
    // `setSubtitle:` is iOS 26; earlier systems show the title alone.
    if wants(ChromeField::Subtitle) && item.respondsToSelector(sel!(setSubtitle:)) {
        let subtitle = bar.subtitle.as_deref().map(NSString::from_str);
        item.setSubtitle(subtitle.as_deref());
    }
    if wants(ChromeField::LargeTitle) {
        page.nav
            .navigationBar()
            .setPrefersLargeTitles(bar.large_title);
        item.setLargeTitleDisplayMode(if bar.large_title {
            UINavigationItemLargeTitleDisplayMode::Always
        } else {
            UINavigationItemLargeTitleDisplayMode::Never
        });
    }
    if wants(ChromeField::Leading) {
        item.setLeftBarButtonItem(leading_item(mtm, &bar.leading, target).as_deref());
    }
    if wants(ChromeField::Actions) {
        // UIKit lists right items from the trailing edge; the spec reads leading-first.
        let items: Vec<Retained<UIBarButtonItem>> = bar
            .actions
            .iter()
            .enumerate()
            .rev()
            .map(|(index, action)| action_item(mtm, index, action, target))
            .collect();
        item.setRightBarButtonItems(Some(&NSArray::from_retained_slice(&items)));
    }
}

/// The leading item: a custom back chevron (the navigation controller has no
/// stack to pop) reporting through `target`, a 32pt round avatar, or a symbol.
fn leading_item(
    mtm: MainThreadMarker,
    leading: &Leading,
    target: &FrustNativeControlTarget,
) -> Option<Retained<UIBarButtonItem>> {
    match leading {
        Leading::None => None,
        Leading::Back => {
            let item = UIBarButtonItem::new(mtm);
            item.setImage(UIImage::systemImageNamed(ns_string!("chevron.left")).as_deref());
            item.setAccessibilityLabel(Some(ns_string!("Back")), mtm);
            target.attach_bar_item(&item, true);
            Some(item)
        }
        Leading::Image(bytes) => {
            let image = UIImage::imageWithData(&NSData::with_bytes(bytes));
            let view = UIImageView::initWithImage(UIImageView::alloc(mtm), image.as_deref());
            view.setContentMode(UIViewContentMode::ScaleAspectFill);
            view.setClipsToBounds(true);
            view.layer().setCornerRadius(AVATAR_POINTS / 2.0);
            view.setTranslatesAutoresizingMaskIntoConstraints(false);
            view.widthAnchor()
                .constraintEqualToConstant(AVATAR_POINTS)
                .setActive(true);
            view.heightAnchor()
                .constraintEqualToConstant(AVATAR_POINTS)
                .setActive(true);
            let item = UIBarButtonItem::initWithCustomView(UIBarButtonItem::alloc(mtm), &view);
            Some(item)
        }
        Leading::Symbol(name) => {
            let item = UIBarButtonItem::new(mtm);
            item.setImage(UIImage::systemImageNamed(&NSString::from_str(name)).as_deref());
            Some(item)
        }
    }
}

/// One trailing action, tagged with its spec index: a pull-down when it has
/// a menu, else a plain image (title as its label) or title item reporting
/// through `target`.
fn action_item(
    mtm: MainThreadMarker,
    index: usize,
    action: &ActionItem,
    target: &FrustNativeControlTarget,
) -> Retained<UIBarButtonItem> {
    let image = action.icon.as_ref().and_then(ui_image);
    let title = NSString::from_str(&action.title);
    let item = match (&action.menu, &image) {
        (Some(entries), Some(image)) => UIBarButtonItem::initWithImage_menu(
            UIBarButtonItem::alloc(mtm),
            Some(image),
            Some(&menu(mtm, index, entries)),
        ),
        (Some(entries), None) => UIBarButtonItem::initWithTitle_menu(
            UIBarButtonItem::alloc(mtm),
            Some(&title),
            Some(&menu(mtm, index, entries)),
        ),
        (None, _) => {
            let item = UIBarButtonItem::new(mtm);
            item.setImage(image.as_deref());
            target.attach_bar_item(&item, false);
            item
        }
    };
    if image.is_some() {
        item.setAccessibilityLabel(Some(&title), mtm);
    } else {
        item.setTitle(Some(&title));
    }
    // Spec indices are small (a top bar's actions); the tag maps a tap back.
    item.setTag(index as isize);
    item
}

/// The pull-down menu over action `action`'s `entries`.
fn menu(mtm: MainThreadMarker, action: usize, entries: &[MenuItem]) -> Retained<UIMenu> {
    let children: Vec<Retained<UIMenuElement>> = entries
        .iter()
        .enumerate()
        .map(|(item, entry)| Retained::into_super(menu_action(mtm, action, item, entry)))
        .collect();
    UIMenu::menuWithChildren(&NSArray::from_retained_slice(&children), mtm)
}

/// Item `item` of action `action`'s menu; its handler reports the choice.
fn menu_action(
    mtm: MainThreadMarker,
    action: usize,
    item: usize,
    entry: &MenuItem,
) -> Retained<UIAction> {
    let handler = RcBlock::new(move |_: NonNull<UIAction>| {
        let tap = || report("menu UIAction", |_| Some(ChromeTap::Menu(action, item)));
        if catch_unwind(AssertUnwindSafe(tap)).is_err() {
            log::warn!("frust-native-widgets: panic caught in a chrome menu handler");
        }
    });
    let image = entry.icon.as_ref().and_then(ui_image);
    // SAFETY: `handler` is a live heap block for the whole call, and UIKit
    // copies (retains) the handler it is given.
    let action = unsafe {
        UIAction::actionWithTitle_image_identifier_handler(
            &NSString::from_str(&entry.title),
            image.as_deref(),
            None,
            RcBlock::as_ptr(&handler),
            mtm,
        )
    };
    let mut attributes = UIMenuElementAttributes::empty();
    if entry.destructive {
        attributes |= UIMenuElementAttributes::Destructive;
    }
    if !entry.enabled {
        attributes |= UIMenuElementAttributes::Disabled;
    }
    action.setAttributes(attributes);
    action
}

/// The `UIImage` for `icon`: an SF Symbol by name, or bytes normalized into
/// the tab-icon point box like the bare tab bar's. `None` when it fails.
fn ui_image(icon: &TabIcon) -> Option<Retained<UIImage>> {
    match icon {
        TabIcon::AppleSymbol(name) => UIImage::systemImageNamed(&NSString::from_str(name)),
        TabIcon::Bytes(bytes) => {
            let data = NSData::with_bytes(bytes);
            let probe = UIImage::imageWithData(&data)?;
            // SAFETY: `size` is marked unsafe only for thread safety; this
            // runs on the main thread against an image only this call owns.
            let size = unsafe { probe.size() };
            let scale = icon_decode_scale(size.width, size.height);
            if scale == 1.0 {
                Some(probe)
            } else {
                UIImage::imageWithData_scale(&data, scale)
            }
        }
    }
}

/// A tap on a chrome page: deliver what it requests, and answer whether
/// UIKit may select `controller` itself (only one that is no chrome page).
pub(super) fn tab_tapped(controller: &UIViewController) -> bool {
    let mut foreign = false;
    report("tab", |chrome| {
        let index = chrome.pages.iter().position(|page| {
            let nav: &UIViewController = &page.nav;
            std::ptr::eq(nav, controller)
        });
        foreign = index.is_none();
        index.map(ChromeTap::Tab)
    });
    foreign
}

/// A tap on the trailing action item tagged `tag`.
pub(super) fn action_tapped(tag: isize) {
    report("action", |_| {
        usize::try_from(tag).ok().map(ChromeTap::Action)
    });
}

/// A tap on the back item.
pub(super) fn back_tapped() {
    report("back", |_| Some(ChromeTap::Back));
}

/// Resolve the tap `locate` finds against the spec on screen, then deliver it
/// with the [`CHROME`] borrow released.
fn report(which: &str, locate: impl FnOnce(&Chrome) -> Option<ChromeTap>) {
    let resolved = CHROME.with(|cell| {
        let state = cell.try_borrow().ok()?;
        let chrome = state.as_ref()?;
        let event = resolve_tap(&chrome.spec, locate(chrome)?)?;
        Some((chrome.generation, event))
    });
    let delivered =
        resolved.is_some_and(|(generation, event)| chrome::deliver_event(generation, &event));
    if !delivered {
        log::debug!("frust-native-widgets: a chrome {which} tap resolved to no event — dropped");
    }
}

/// A page's state: the frust controller and host view while it holds them.
#[derive(Default)]
struct PageIvars {
    contained: RefCell<Option<Retained<UIViewController>>>,
    host: RefCell<Option<Retained<UIView>>>,
}

define_class!(
    // SAFETY:
    // - `UIViewController` is designed for subclassing; the overrides call
    //   super where UIKit requires it and return what UIKit expects.
    // - The ivars are plain Rust values; the generated `dealloc` drops them
    //   without touching the object being deallocated.
    #[unsafe(super(UIViewController, UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = PageIvars]
    struct FrustChromePageController;

    unsafe impl NSObjectProtocol for FrustChromePageController {}

    impl FrustChromePageController {
        #[unsafe(method_id(childViewControllerForStatusBarHidden))]
        fn child_for_status_bar_hidden(&self) -> Option<Retained<UIViewController>> {
            self.contained()
        }

        #[unsafe(method_id(childViewControllerForHomeIndicatorAutoHidden))]
        fn child_for_home_indicator_auto_hidden(&self) -> Option<Retained<UIViewController>> {
            self.contained()
        }

        /// Pins the host view to the page's bounds.
        #[unsafe(method(viewDidLayoutSubviews))]
        fn view_did_layout_subviews(&self) {
            // SAFETY: forwarding the same no-argument selector to
            // `UIViewController`'s implementation, as UIKit expects.
            let _: () = unsafe { msg_send![super(self), viewDidLayoutSubviews] };
            let pinned = catch_unwind(AssertUnwindSafe(|| {
                let host = self.ivars().host.borrow().clone();
                if let (Some(host), Some(view)) = (host, self.viewIfLoaded()) {
                    host.setFrame(view.bounds());
                }
            }));
            if pinned.is_err() {
                log::error!("frust-native-widgets: a chrome page layout step panicked");
            }
        }
    }
);

impl FrustChromePageController {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PageIvars::default());
        // SAFETY: `UIViewController`'s `init` on a freshly allocated
        // instance whose ivars are set (`apple_sheet`'s controller idiom).
        unsafe { msg_send![super(this), init] }
    }

    /// The contained frust controller, panic-free for UIKit's callers.
    fn contained(&self) -> Option<Retained<UIViewController>> {
        self.ivars()
            .contained
            .try_borrow()
            .ok()
            .and_then(|contained| contained.clone())
    }

    fn holds(&self) -> bool {
        self.contained().is_some()
    }

    /// Contain `frust`, adding `host` (the wrapped `FrustView`) to this view.
    fn hold(&self, frust: &UIViewController, host: &UIView) {
        self.addChildViewController(frust);
        if let Some(view) = self.view() {
            host.setFrame(view.bounds());
            view.addSubview(host);
        }
        frust.didMoveToParentViewController(Some(self));
        self.ivars().contained.replace(Some(frust.retain()));
        self.ivars().host.replace(Some(host.retain()));
    }

    /// Give up the frust controller, if held; the host view leaves too.
    fn release(&self) {
        let Some(frust) = self.ivars().contained.take() else {
            return;
        };
        frust.willMoveToParentViewController(None);
        if let Some(host) = self.ivars().host.take() {
            host.removeFromSuperview();
        }
        frust.removeFromParentViewController();
    }
}
