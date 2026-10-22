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
//! # Failures after `install` answered `Ok`
//!
//! The queued half re-checks the precondition (an alert queued on the same
//! main queue may present first) and may panic; either way it records the
//! failure for its generation in a lock-free cell (`FAILURE`) that
//! [`IosChromeHost::take_failure`] hands the slot on its next publish, then
//! bumps the chrome epoch so the scaffold republishes and draws the fallback
//! bars. A panic in an apply disposes the half-applied chrome (re-rooting the
//! window to the frust controller) and records a refusal; a panic mid-install
//! restores the window's original root first. A NoHost, synchronous or
//! deferred, starts bounded re-checks of the precondition on later main-queue
//! turns (100 ms doubling to a 2 s interval) that bump the epoch once it
//! passes, so the slot's retry installs without waiting for an unrelated
//! rebuild; they stop when the generation is withdrawn, installed or replaced.
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
//! `Send` and const-constructible for the slot's `static`. Only a tab tap moves
//! the frust controller inside a UIKit callback, the tab bar controller
//! delegate's, which no frust frame encloses (Selection below).
//!
//! # The containment-move contract
//!
//! The frust controller's view (the `FrustView`) is wrapped in a plugin-owned
//! host `UIView`; the controller is a child of exactly one page at a time,
//! and the host is only ever in that page's view (its current parent's) or
//! in no view at all: UIKit raises a hierarchy-inconsistency exception for a
//! child controller's view under another controller's view, and a foreign
//! exception aborts the process. A selection change moves the controller in
//! one turn: the old page lets it go (`willMoveToParentViewController(nil)`,
//! host removed, `removeFromParentViewController`), leaving the host with no
//! superview; the new page takes it (`addChildViewController`,
//! `didMoveToParentViewController`) and adds the host to its view once it is
//! in the window, from `viewIsAppearing:` (iOS 17+; the deployment target is
//! 15.0, so the method is `respondsToSelector`-guarded and earlier systems
//! rely on `viewDidLayoutSubviews`/`viewDidAppear:`, which place it too).
//! Pages are kept by `TabId`, so a reorder never loses the contained controller.
//!
//! # Selection
//!
//! A tap on another chrome page moves the controller inside
//! `tabBarController:shouldSelectViewController:` and answers YES, so UIKit
//! selects natively (the Liquid Glass lens slides without a bounce). An
//! app-driven change (`ChromeSpec::selected`) snapshots the host while on
//! screen, moves it, calls `setSelectedIndex` inside `performWithoutAnimation`
//! (iOS 26 animates it otherwise, the page entering the window only at the
//! end), forces a `layoutIfNeeded` of the tab bar and navigation controllers'
//! views, then places the host. A page still out of the window places it
//! later; until then the snapshot (a plain view, no controller) covers the
//! host's last frame just below the tab bar in the tab bar controller's view,
//! removed once the host is placed or after 0.5 s at the latest. A tab change
//! that drops the holding page snapshots before letting it go.
//!
//! A native tap stays pending for [`SETTLE_WINDOW`] against applies published
//! before the app saw it ([`settle`]): one still naming the tab the tap left is
//! stale and keeps the bar, one naming the tapped tab confirms it, a third tab
//! is honoured at once. At the deadline the bar always converges to the tab
//! the app's spec names ([`expire`]): a spec naming any tab other than the
//! newest tapped one (the left tab, a tab passed on the way, or another)
//! moves the bar there the same unanimated way, since the app rejected the
//! newest tap; a spec naming the tapped tab, or none, keeps it.
//! A newer tap inside the window chains onto the pending one ([`chain`]): it
//! keeps the originally confirmed tab as `from`, so an apply confirming an
//! earlier tap holds rather than snapping off the newest one, and while a tap
//! is pending a tab tap resolves against the tab UIKit shows ([`tap_spec`]).
//! A tab-list change drops the pending tap, and the timer acts only on its
//! own generation and tap.
//!
//! Every page view and the tab bar controller's view carry a backdrop,
//! `systemBackgroundColor` resolved for `ChromeSpec::dark` into a concrete
//! colour, so a frame the host has not reached shows the theme's background
//! rather than black; it is repainted where the bars are restyled.
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
//! # Toolbar
//!
//! `ChromeSpec::toolbar` is global, so every page controller carries its own
//! `toolbarItems` (secondary items with flexible space between them, then the
//! primary trailing in `UIBarButtonItemStyle::Prominent`) and every navigation
//! controller shows its toolbar (`setToolbarHidden:animated:NO`), so a tab
//! switch never animates it in or out; an absent or empty toolbar hides it on
//! all of them. It rides `ChromeField::Actions` (the slot's one action index
//! space) and is re-applied to every page on each such change. `Prominent`
//! shares its raw value with the older `Done`, so it needs no availability
//! check: iOS 26 draws a separated tinted button, earlier systems a bold title.
//! The navigation controller insets its page's safe area by the toolbar as it
//! does for the tab bar, so the frust body's bottom view padding (and the
//! scaffold's edge fade keyed off it) covers both.
//!
//! # Events
//!
//! One `FrustNativeControlTarget` per install, held in the chrome state, is
//! the tab bar controller's delegate and the target of the back, plain action
//! and toolbar items (set as each item is built), tagged with their
//! `crate::chrome::chrome_actions` index; a menu item's `UIAction` block
//! captures its action and item indices. Each tap is resolved against the
//! spec UIKit shows (`crate::chrome::resolve_tap`, a tab against the
//! app-confirmed `selected`, or the pending tap's tab while one is pending)
//! and handed to `crate::chrome::deliver_event`,
//! never with a [`CHROME`] borrow held (`try_borrow`: an apply in progress
//! has taken the state, the tap is dropped and the delegate answers NO). A tap
//! on the shown tab reports `TabReselected` and answers NO; one on another
//! chrome page reports `TabSelected` and answers YES (Selection above); a
//! foreign controller (the system "More" list) answers YES.
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
//! feed the app theme's brightness and would pin it (hence the resolved
//! backdrop colour, not a page override). The bars are restyled on install,
//! for every new page, and on any apply whose spec flips it.
//!
//! # Limitations
//!
//! `native-chrome-single-surface-containment`: one frust surface serves every
//! tab, and the navigation controllers have no stack (no native push
//! animation; `Leading::Back` is a custom item reporting `BackPressed`).

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchTime};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_core_foundation::{CGFloat, CGRect};
use objc2_foundation::{NSArray, NSData, NSString, ns_string};
use objc2_ui_kit::{
    NSObjectUIAccessibility, UIAction, UIApplication, UIBarButtonItem, UIBarButtonItemStyle,
    UIColor, UIImage, UIImageView, UIMenu, UIMenuElement, UIMenuElementAttributes,
    UINavigationController, UINavigationItemLargeTitleDisplayMode, UIResponder,
    UISceneActivationState, UITabBarController, UITraitCollection, UIUserInterfaceStyle, UIView,
    UIViewAutoresizing, UIViewContentMode, UIViewController, UIWindow, UIWindowScene,
};

use super::FrustNativeControlTarget;
use super::theme::apply_user_interface_style;
use crate::api::{TabIcon, TabId, TabItem};
use crate::chrome::host::ChromeField;
use crate::chrome::{
    self, ActionItem, ChromeDiff, ChromeError, ChromeGeneration, ChromeHost, ChromeSpec, ChromeTap,
    Leading, MenuItem, TopBarSpec, resolve_tap, shown_toolbar, toolbar_offset,
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
        let generation = generation.get();
        let checked = MainThreadMarker::new()
            .ok_or(ChromeError::NoHost)
            .and_then(check_host);
        if let Err(error) = checked {
            if error == ChromeError::NoHost {
                schedule_recheck(generation);
            } else {
                stop_recheck(generation);
            }
            return Err(error);
        }
        // Any re-check chain ends: this install supersedes it.
        RECHECK.store(0, Ordering::Release);
        INSTALLING.store(generation, Ordering::Release);
        let spec = spec.clone();
        self.live = Some(generation);
        on_main(move |mtm| {
            if catch_unwind(AssertUnwindSafe(|| install_now(mtm, generation, &spec))).is_err() {
                fail(generation, Failure::Panicked);
            }
        });
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
            with_chrome(mtm, generation, |chrome| chrome.apply(mtm, &diff));
        });
    }

    fn dispose(&mut self) {
        if let Some(generation) = self.live.take() {
            forget(generation);
            on_main(move |mtm| {
                if let Some(chrome) = take_chrome(generation) {
                    chrome.dispose(mtm);
                }
            });
        }
    }

    /// Lock-free: reads and clears [`FAILURE`]. Taking a failure forgets the
    /// install, so a later dispose never targets the dead generation.
    fn take_failure(&mut self, generation: ChromeGeneration) -> Option<ChromeError> {
        let generation = generation.get();
        let held = FAILURE.load(Ordering::Acquire);
        let failure = Failure::decode(held)?;
        let failed = held >> FAILURE_KIND_BITS;
        if failed > generation {
            return None;
        }
        // Ours, or an older generation's that no one will ask for: spent.
        FAILURE
            .compare_exchange(held, 0, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        if failed != generation {
            return None;
        }
        if self.live == Some(generation) {
            self.live = None;
        }
        Some(failure.error())
    }

    fn abandon(&mut self, generation: ChromeGeneration) {
        forget(generation.get());
    }
}

/// Why a deferred step failed: the low [`FAILURE_KIND_BITS`] of [`FAILURE`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    /// No window, or the root presents a modal: retried.
    NoHost = 1,
    /// The root became a container: the fallback is pinned.
    Rerooted = 2,
    /// A UIKit step panicked: the fallback is pinned.
    Panicked = 3,
}

const FAILURE_KIND_BITS: u32 = 2;

impl Failure {
    fn decode(held: u64) -> Option<Self> {
        match held & ((1 << FAILURE_KIND_BITS) - 1) {
            1 => Some(Self::NoHost),
            2 => Some(Self::Rerooted),
            3 => Some(Self::Panicked),
            _ => None,
        }
    }

    fn error(self) -> ChromeError {
        match self {
            Self::NoHost => ChromeError::NoHost,
            Self::Rerooted => ChromeError::Refused("window already re-rooted".into()),
            Self::Panicked => ChromeError::Refused("a native chrome step panicked".into()),
        }
    }
}

/// The failure channel's cell: the failed generation shifted past the
/// [`Failure`] kind, `0` for none. Written on the main queue and taken under
/// the slot lock, both lock-free; generations stay far below `2^62`.
static FAILURE: AtomicU64 = AtomicU64::new(0);
/// The generation the last accepted install was for; `0` once it is gone.
static INSTALLING: AtomicU64 = AtomicU64::new(0);
/// The generation a NoHost re-check chain runs for; `0` for none. A chain
/// whose generation this no longer names stops at its next turn.
static RECHECK: AtomicU64 = AtomicU64::new(0);

/// The first NoHost re-check's delay; each later one doubles, up to
/// [`RECHECK_MAX`].
const RECHECK_FIRST: Duration = Duration::from_millis(100);
const RECHECK_MAX: Duration = Duration::from_secs(2);
/// The longest a [`Cover`] stays up when no page places the host.
const COVER_MAX: Duration = Duration::from_millis(500);
/// How long a native tab tap waits for the app's confirmation (module doc,
/// Selection).
const SETTLE_WINDOW: Duration = Duration::from_millis(250);

/// Record `failure` for `generation`; an older generation's never replaces
/// a newer one's.
fn record_failure(generation: u64, failure: Failure) {
    let packed = (generation << FAILURE_KIND_BITS) | failure as u64;
    let _ = FAILURE.fetch_update(Ordering::AcqRel, Ordering::Acquire, |held| {
        (held >> FAILURE_KIND_BITS <= generation).then_some(packed)
    });
}

/// The main-queue failure path: record `failure` for `generation`, re-check
/// the host for a NoHost while that install is still the current one, then
/// bump the chrome epoch so the scaffold republishes and the slot takes it.
fn fail(generation: u64, failure: Failure) {
    log::warn!(
        "frust-native-widgets: native chrome generation {generation} failed after install \
         ({failure:?}); falling back to frust-drawn chrome"
    );
    record_failure(generation, failure);
    if failure == Failure::NoHost && INSTALLING.load(Ordering::Acquire) == generation {
        schedule_recheck(generation);
    }
    chrome::bump_epoch();
}

/// `generation` was disposed or withdrawn: stop what still runs for it.
fn forget(generation: u64) {
    let _ = INSTALLING.compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire);
    stop_recheck(generation);
}

fn stop_recheck(generation: u64) {
    let _ = RECHECK.compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire);
}

/// Start re-checking the host for `generation`, unless a chain already runs
/// for it (each NoHost publish lands here, at frame rate under a scroll).
fn schedule_recheck(generation: u64) {
    if RECHECK.swap(generation, Ordering::AcqRel) != generation {
        recheck_after(generation, RECHECK_FIRST);
    }
}

/// Queue one [`recheck`] for `generation` after `delay`.
fn recheck_after(generation: u64, delay: Duration) {
    let when = DispatchTime::try_from(delay).unwrap_or(DispatchTime::NOW);
    let queued = DispatchQueue::main().after(when, move || {
        // SAFETY: a block submitted to the main queue runs on the main
        // thread, as in [`on_main`].
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        if catch_unwind(AssertUnwindSafe(|| recheck(mtm, generation, delay))).is_err() {
            log::error!("frust-native-widgets: a native chrome host re-check panicked");
            stop_recheck(generation);
        }
    });
    if queued.is_err() {
        stop_recheck(generation);
    }
}

/// One NoHost re-check. Stops once [`RECHECK`] moved on (withdrawn, installed
/// or replaced). When the precondition passes it bumps the epoch, so the
/// scaffold republishes and the slot's NoHost retry installs — but only once
/// the slot has taken the deferred failure, or that publish would spend the
/// wake falling back. Still no host: check again, the interval doubling up
/// to [`RECHECK_MAX`]. Refused: stop; the next retry pins the fallback.
fn recheck(mtm: MainThreadMarker, generation: u64, delay: Duration) {
    if RECHECK.load(Ordering::Acquire) != generation {
        return;
    }
    match check_host(mtm) {
        Ok(()) if FAILURE.load(Ordering::Acquire) >> FAILURE_KIND_BITS == generation => {
            recheck_after(generation, delay);
        }
        Ok(()) => {
            if RECHECK
                .compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                chrome::bump_epoch();
            }
        }
        Err(ChromeError::NoHost) => recheck_after(generation, (delay * 2).min(RECHECK_MAX)),
        Err(_) => stop_recheck(generation),
    }
}

thread_local! {
    /// The installed chrome, if any. Main thread only.
    static CHROME: RefCell<Option<Chrome>> = const { RefCell::new(None) };
    /// The shown cover and its sequence number. Main thread only.
    static COVER: RefCell<Option<(u64, Retained<UIView>)>> = const { RefCell::new(None) };
    static COVER_SEQ: Cell<u64> = const { Cell::new(0) };
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
/// A panic in `f` disposes the half-applied chrome (re-rooting the window to
/// the frust controller) and records [`Failure::Panicked`], so the slot falls
/// back instead of applying to a chrome that is gone.
fn with_chrome(mtm: MainThreadMarker, generation: u64, f: impl FnOnce(&mut Chrome)) {
    let Some(mut chrome) = take_chrome(generation) else {
        return;
    };
    if catch_unwind(AssertUnwindSafe(|| f(&mut chrome))).is_ok() {
        CHROME.with(|cell| *cell.borrow_mut() = Some(chrome));
        return;
    }
    log::error!("frust-native-widgets: a native chrome apply panicked; disposing the chrome");
    if catch_unwind(AssertUnwindSafe(|| chrome.dispose(mtm))).is_err() {
        log::error!("frust-native-widgets: disposing the panicked native chrome panicked too");
    }
    fail(generation, Failure::Panicked);
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

/// The queued half of [`IosChromeHost::install`]: re-checks the
/// precondition, which a modal presented since may break, and records a
/// failure ([`fail`]) instead of installing: NoHost for no window or a
/// presented modal, a refusal for a container root or a panic mid-install,
/// after restoring the window's original root.
fn install_now(mtm: MainThreadMarker, generation: u64, spec: &ChromeSpec) {
    if let Some(stale) = take_chrome_matching(None) {
        stale.dispose(mtm);
    }
    let root = anchor_window(mtm).and_then(|window| Some((window.rootViewController()?, window)));
    let Some((frust, window)) = root else {
        return fail(generation, Failure::NoHost);
    };
    if is_container(&frust) {
        return fail(generation, Failure::Rerooted);
    }
    if frust.presentedViewController().is_some() {
        return fail(generation, Failure::NoHost);
    }
    let stack = sibling_stack(&frust);
    let original = frust.clone();
    let built = catch_unwind(AssertUnwindSafe(|| {
        Chrome::install(mtm, generation, &window, frust, spec, &stack)
    }));
    match built {
        Ok(chrome) => CHROME.with(|cell| *cell.borrow_mut() = Some(chrome)),
        Err(_) => {
            log::error!("frust-native-widgets: native chrome install panicked; restoring the root");
            let restored = catch_unwind(AssertUnwindSafe(|| {
                restore_root(&window, &original, &stack);
            }));
            if restored.is_err() {
                log::error!("frust-native-widgets: restoring the window root panicked");
            }
            fail(generation, Failure::Panicked);
        }
    }
}

/// The views sharing the frust view's superview, bottom to top (the frust
/// view among them), which install moves into the host view.
fn sibling_stack(frust: &UIViewController) -> Vec<Retained<UIView>> {
    frust
        .view()
        .and_then(|view| view.superview())
        .map(|superview| superview.subviews().iter().collect())
        .unwrap_or_default()
}

/// Undo a half-done install: free the frust controller from any page, root
/// the window at it again and put `stack` back around its view.
fn restore_root(window: &UIWindow, frust: &UIViewController, stack: &[Retained<UIView>]) {
    detach_from_parent(frust);
    let rooted = window
        .rootViewController()
        .is_some_and(|root| std::ptr::eq(&*root, frust));
    if !rooted {
        window.setRootViewController(Some(frust));
    }
    if let Some(view) = frust.view()
        && let Some(superview) = view.superview()
    {
        restore_siblings(stack, &view, &superview);
    }
}

/// Take `frust` out of whatever page still contains it (one a panic left
/// holding it), so the window can root it again.
fn detach_from_parent(frust: &UIViewController) {
    if frust.parentViewController().is_some() {
        frust.willMoveToParentViewController(None);
        frust.removeFromParentViewController();
    }
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
    /// The native tab tap the app has not confirmed yet (module doc, Selection).
    pending: Option<NativeSelect>,
    /// Native tab taps so far: the last one's number.
    taps: u64,
}

impl Chrome {
    /// `stack` is [`sibling_stack`]'s, taken by the caller so a panic here
    /// can put it back.
    fn install(
        mtm: MainThreadMarker,
        generation: u64,
        window: &Retained<UIWindow>,
        frust: Retained<UIViewController>,
        spec: &ChromeSpec,
        stack: &[Retained<UIView>],
    ) -> Self {
        let frust_view = frust.view();
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
            pending: None,
            taps: 0,
        };
        chrome.target.attach_tab_bar_controller(&chrome.tabs);
        let cover = chrome.sync_pages(mtm, spec);
        apply_user_interface_style(&chrome.tabs.tabBar(), spec.dark);
        paint_backdrop(chrome.tabs.view(), spec.dark);
        // The root swap takes the `FrustView` out of the window; its siblings
        // stay until moved, in their old order, into the host view.
        window.setRootViewController(Some(&chrome.tabs));
        for view in stack {
            chrome.host.addSubview(view);
        }
        if let Some(view) = frust_view {
            view.setFrame(chrome.host.bounds());
            view.setAutoresizingMask(flexible);
        }
        // The former window root has no parent and the fresh host no
        // superview; detaching keeps that true before a page holds it.
        detach_from_parent(&chrome.frust);
        chrome.select(spec, cover);
        chrome
    }

    /// `ChromeField::Scroll` is ignored here: the iOS bars do not react to the
    /// body's scroll (the scaffold draws the edge fade instead).
    fn apply(&mut self, mtm: MainThreadMarker, diff: &ChromeDiff<'_>) {
        let spec = diff.spec;
        let restyle = self.spec.dark != spec.dark;
        self.spec = spec.clone();
        let cover = if diff.contains(ChromeField::Tabs) {
            // The pending tap's page indices may name other pages now.
            self.pending = None;
            self.sync_pages(mtm, spec)
        } else {
            None
        };
        if diff.contains(ChromeField::Tabs) || diff.contains(ChromeField::Selected) {
            let wanted = selected_index(spec);
            match settle(self.pending.as_ref(), wanted, Instant::now()) {
                Settle::Follow => self.select(spec, cover),
                Settle::Hold => log::debug!(
                    "frust-native-widgets: chrome selection {wanted:?} predates a native tab tap; \
                     the bar stays"
                ),
                Settle::Resolve => {
                    self.pending = None;
                    self.select(spec, cover);
                }
            }
        }
        for page in &self.pages {
            let top = self.spec.top.as_ref();
            apply_top(mtm, page, top, &self.target, |field| diff.contains(field));
            if diff.contains(ChromeField::Actions) {
                apply_toolbar(mtm, page, &self.spec, &self.target);
            }
        }
        if restyle {
            apply_user_interface_style(&self.tabs.tabBar(), spec.dark);
            paint_backdrop(self.tabs.view(), spec.dark);
            for page in &self.pages {
                apply_user_interface_style(&page.nav.navigationBar(), spec.dark);
                paint_backdrop(page.page.view(), spec.dark);
            }
        }
    }

    /// Rebuild the view-controller array for `spec.tabs`, keeping pages by id.
    /// Dropping the holding page detaches the host; its snapshot, taken first,
    /// is returned for the [`Self::select`] that follows in the same turn.
    fn sync_pages(&mut self, mtm: MainThreadMarker, spec: &ChromeSpec) -> Option<Cover> {
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
                    apply_toolbar(mtm, &page, &self.spec, &self.target);
                    apply_user_interface_style(&page.nav.navigationBar(), self.spec.dark);
                    paint_backdrop(page.page.view(), self.spec.dark);
                    page
                }
            };
            if let Some(tab) = tab {
                page.set_tab_item(tab);
            }
            self.pages.push(page);
        }
        let cover = old
            .iter()
            .any(|page| page.page.holds())
            .then(|| self.snapshot_cover())
            .flatten();
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
        cover
    }

    /// Select `spec.selected` (an unknown id keeps the current tab) through
    /// [`Self::show_page`].
    fn select(&self, spec: &ChromeSpec, cover: Option<Cover>) {
        let current = self.tabs.selectedIndex();
        self.show_page(selected_index(spec).unwrap_or(current), cover);
    }

    /// Select page `index` (page 0 when out of range) without animation and
    /// move the frust controller into it in this one turn (module doc,
    /// Selection). `cover` is a snapshot [`Self::sync_pages`] took before
    /// detaching the host; otherwise one is taken here.
    fn show_page(&self, index: usize, cover: Option<Cover>) {
        let index = if index < self.pages.len() { index } else { 0 };
        let Some(target) = self.pages.get(index) else {
            return;
        };
        if target.page.holds() {
            self.select_tab(index);
        } else {
            let cover = cover.or_else(|| self.snapshot_cover());
            for page in &self.pages {
                page.page.release();
            }
            target.page.hold(&self.frust, &self.host);
            self.select_tab(index);
            if let Some(view) = self.tabs.view() {
                view.layoutIfNeeded();
            }
            if let Some(view) = target.nav.view() {
                view.layoutIfNeeded();
            }
            let covered = !target.page.place_host() && self.show_cover(cover);
            let in_window = self.host.window().is_some();
            log::debug!(
                "frust-native-widgets: chrome page {index} selected; host in window: \
                 {in_window}, cover shown: {covered}"
            );
            debug_assert!(
                in_window || covered,
                "the chrome host is off screen, uncovered"
            );
        }
        self.tabs.setNeedsStatusBarAppearanceUpdate();
        self.tabs.setNeedsUpdateOfHomeIndicatorAutoHidden();
    }

    /// `setSelectedIndex` inside `performWithoutAnimation`: iOS 26 animates a
    /// programmatic selection, putting the page in the window only at its end.
    fn select_tab(&self, index: usize) {
        let tabs = self.tabs.clone();
        let block = RcBlock::new(move || tabs.setSelectedIndex(index));
        UIView::performWithoutAnimation(&block, self.tabs.mtm());
    }

    /// The page whose navigation controller is `controller`.
    fn page_of(&self, controller: &UIViewController) -> Option<usize> {
        self.pages.iter().position(|page| {
            let nav: &UIViewController = &page.nav;
            std::ptr::eq(nav, controller)
        })
    }

    /// A tap on `controller`'s tab: when it is another chrome page, move the
    /// frust controller there (the host has no superview until that page
    /// places it), record the tap as pending and start its settle timer.
    /// Whether UIKit may select it natively.
    fn native_select(&mut self, controller: &UIViewController) -> bool {
        let Some(to) = self.page_of(controller) else {
            return false;
        };
        let from = self.tabs.selectedIndex();
        if from == to {
            return false;
        }
        for page in &self.pages {
            page.page.release();
        }
        self.pages[to].page.hold(&self.frust, &self.host);
        self.taps += 1;
        let tap = self.taps;
        let deadline = Instant::now() + SETTLE_WINDOW;
        self.pending = chain(
            self.pending.as_ref(),
            from,
            to,
            self.pages.len(),
            deadline,
            tap,
        );
        if self.pending.is_some() {
            settle_after(self.generation, tap);
        }
        self.tabs.setNeedsStatusBarAppearanceUpdate();
        self.tabs.setNeedsUpdateOfHomeIndicatorAutoHidden();
        true
    }

    /// Native tap `tap`'s deadline: move the bar to the spec's page when the
    /// app never confirmed the newest tap ([`expire`]).
    fn settle_expired(&mut self, tap: u64) {
        match expire(self.pending.as_ref(), tap, selected_index(&self.spec)) {
            Expiry::Superseded => {}
            Expiry::Settled => self.pending = None,
            Expiry::SnapBack(page) => {
                self.pending = None;
                log::debug!(
                    "frust-native-widgets: unconfirmed chrome tab tap; to the spec's page {page}"
                );
                self.show_page(page, None);
            }
        }
    }

    /// The host as it shows now and its frame in the tab bar controller's
    /// view; `None` when it is not in the window.
    fn snapshot_cover(&self) -> Option<Cover> {
        let tabs = self.tabs.view()?;
        self.host.window()?;
        let view = self.host.snapshotViewAfterScreenUpdates(false)?;
        let frame = self
            .host
            .convertRect_toView(self.host.bounds(), Some(&tabs));
        Some(Cover { view, frame })
    }

    /// Put `cover` (a backdrop-painted view over the whole tab bar controller
    /// when `None`) in the tab bar controller's view just below the tab bar,
    /// until a page places the host or [`COVER_MAX`] passes. Whether it is up.
    fn show_cover(&self, cover: Option<Cover>) -> bool {
        let Some(tabs) = self.tabs.view() else {
            return false;
        };
        let cover = cover.unwrap_or_else(|| {
            let view = UIView::new(self.tabs.mtm());
            paint_backdrop(Some(view.clone()), self.spec.dark);
            Cover {
                view,
                frame: tabs.bounds(),
            }
        });
        cover.view.setFrame(cover.frame);
        match child_holding(&tabs, &self.tabs.tabBar()) {
            Some(bar) => tabs.insertSubview_belowSubview(&cover.view, &bar),
            None => tabs.addSubview(&cover.view),
        }
        keep_cover(cover.view);
        true
    }

    /// Re-root the window back to the frust controller and release the rest.
    /// Tolerates a half-applied chrome: a page a panicking `sync_pages` let
    /// go of may still contain the frust controller, which is detached.
    fn dispose(self, _mtm: MainThreadMarker) {
        remove_cover(None);
        self.target.detach_tab_bar_controller(&self.tabs);
        let stack: Vec<Retained<UIView>> = self.host.subviews().iter().collect();
        for page in &self.pages {
            page.page.release();
        }
        detach_from_parent(&self.frust);
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

fn is_superview(view: &UIView, parent: &UIView) -> bool {
    view.superview()
        .is_some_and(|superview| std::ptr::eq(&*superview, parent))
}

/// `view`'s ancestor (itself included) that is a direct subview of `parent`.
fn child_holding(parent: &UIView, view: &UIView) -> Option<Retained<UIView>> {
    let mut child = view.retain();
    loop {
        let superview = child.superview()?;
        if std::ptr::eq(&*superview, parent) {
            return Some(child);
        }
        child = superview;
    }
}

/// A plain view (no controller) standing in for the host while the selected
/// page is off-window, and its frame in the tab bar controller's view.
struct Cover {
    view: Retained<UIView>,
    frame: CGRect,
}

/// Record `view` as the shown cover, replacing any older one, and remove it
/// after [`COVER_MAX`] unless a page placed the host first.
fn keep_cover(view: Retained<UIView>) {
    let seq = COVER_SEQ.with(|cell| {
        cell.set(cell.get() + 1);
        cell.get()
    });
    let old = COVER.with(|cell| cell.replace(Some((seq, view))));
    if let Some((_, old)) = old {
        old.removeFromSuperview();
    }
    let when = DispatchTime::try_from(COVER_MAX).unwrap_or(DispatchTime::NOW);
    let queued = DispatchQueue::main().after(when, move || {
        if catch_unwind(AssertUnwindSafe(|| remove_cover(Some(seq)))).is_err() {
            log::error!("frust-native-widgets: removing a chrome cover panicked");
        }
    });
    if queued.is_err() {
        remove_cover(Some(seq));
    }
}

/// Remove the shown cover: any when `only` is `None`, else only cover `only`.
fn remove_cover(only: Option<u64>) {
    let cover = COVER.with(|cell| {
        let mut slot = cell.try_borrow_mut().ok()?;
        match (slot.as_ref(), only) {
            (Some((seq, _)), Some(wanted)) if *seq != wanted => None,
            _ => slot.take(),
        }
    });
    if let Some((_, view)) = cover {
        view.removeFromSuperview();
    }
}

/// The page `spec.selected` names, if any.
fn selected_index(spec: &ChromeSpec) -> Option<usize> {
    let id = spec.selected.as_ref()?;
    spec.tabs.iter().position(|tab| tab.id() == id)
}

/// A tab tap UIKit selected natively that no apply has confirmed yet.
#[derive(Clone, Debug, PartialEq, Eq)]
struct NativeSelect {
    /// The page shown before the first unconfirmed tap.
    from: usize,
    /// The newest tapped page, the one UIKit shows.
    to: usize,
    /// Pages tapped in between: an apply naming one confirms an older tap.
    passed: Vec<usize>,
    /// When an unconfirmed tap snaps back.
    deadline: Instant,
    /// The newest tap's number ([`Chrome::taps`]): a timer acts only on it.
    tap: u64,
}

/// The pending state after a native tap moves UIKit from page `shown` to
/// page `to`: chained onto `previous` (keeping its `from`) when a tap is
/// still pending, else fresh from `shown` — or `None` when `shown` is no
/// chrome page (the "More" list), with nothing to snap back to.
fn chain(
    previous: Option<&NativeSelect>,
    shown: usize,
    to: usize,
    pages: usize,
    deadline: Instant,
    tap: u64,
) -> Option<NativeSelect> {
    let Some(previous) = previous else {
        return (shown < pages).then(|| NativeSelect {
            from: shown,
            to,
            passed: Vec::new(),
            deadline,
            tap,
        });
    };
    let mut passed = previous.passed.clone();
    if !passed.contains(&previous.to) {
        passed.push(previous.to);
    }
    passed.retain(|&page| page != previous.from && page != to);
    Some(NativeSelect {
        from: previous.from,
        to,
        passed,
        deadline,
        tap,
    })
}

/// The spec a tap resolves against: `spec`, with `selected` moved to the
/// pending tap's page — the tab UIKit shows — while one is pending.
fn tap_spec<'a>(spec: &'a ChromeSpec, pending: Option<&NativeSelect>) -> Cow<'a, ChromeSpec> {
    let Some(tab) = pending.and_then(|pending| spec.tabs.get(pending.to)) else {
        return Cow::Borrowed(spec);
    };
    let mut shown = spec.clone();
    shown.selected = Some(tab.id().clone());
    Cow::Owned(shown)
}

/// What an apply does with the page its spec selects ([`settle`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Settle {
    /// No tap pending: select the spec's page.
    Follow,
    /// The spec predates the newest tap or names no page: keep the bar and the tap.
    Hold,
    /// Select the spec's page and drop the tap: confirmed, overridden or expired.
    Resolve,
}

/// The settle decision for an apply selecting page `wanted` at `now`.
fn settle(pending: Option<&NativeSelect>, wanted: Option<usize>, now: Instant) -> Settle {
    let Some(pending) = pending else {
        return Settle::Follow;
    };
    if now >= pending.deadline {
        return Settle::Resolve;
    }
    match wanted {
        Some(index) if index == pending.to => Settle::Resolve,
        Some(index) if index == pending.from || pending.passed.contains(&index) => Settle::Hold,
        Some(_) => Settle::Resolve,
        None => Settle::Hold,
    }
}

/// What tap `tap`'s deadline does ([`expire`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expiry {
    /// A newer tap, an apply or a tab-list change already settled this one.
    Superseded,
    /// The spec selects the page the bar shows, or names no page: drop the
    /// tap, keep the bar.
    Settled,
    /// The spec selects another page (the one left, one passed or any other):
    /// the app rejected the newest tap, so the bar moves to the spec's page.
    SnapBack(usize),
}

/// The deadline decision for tap `tap` while the last spec selects `wanted`:
/// the bar converges to the page the spec names.
fn expire(pending: Option<&NativeSelect>, tap: u64, wanted: Option<usize>) -> Expiry {
    match (pending, wanted) {
        (Some(pending), Some(page)) if pending.tap == tap && page != pending.to => {
            Expiry::SnapBack(page)
        }
        (Some(pending), _) if pending.tap == tap => Expiry::Settled,
        _ => Expiry::Superseded,
    }
}

/// Queue native tap `tap`'s deadline on the main queue, for `generation`'s
/// chrome only. Unqueued, the tap settles at the first apply past it.
fn settle_after(generation: u64, tap: u64) {
    let when = DispatchTime::try_from(SETTLE_WINDOW).unwrap_or(DispatchTime::NOW);
    let queued = DispatchQueue::main().after(when, move || {
        // SAFETY: a block submitted to the main queue runs on the main
        // thread, as in [`on_main`].
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let expired = || with_chrome(mtm, generation, |chrome| chrome.settle_expired(tap));
        if catch_unwind(AssertUnwindSafe(expired)).is_err() {
            log::error!("frust-native-widgets: settling a chrome tab tap panicked");
        }
    });
    if queued.is_err() {
        log::warn!("frust-native-widgets: a chrome tab tap's settle timer was not queued");
    }
}

/// Paint `view`'s backdrop: `systemBackgroundColor` resolved for `dark` into
/// a concrete colour, no `overrideUserInterfaceStyle` (module doc, Brightness).
fn paint_backdrop(view: Option<Retained<UIView>>, dark: bool) {
    let Some(view) = view else {
        return;
    };
    let style = if dark {
        UIUserInterfaceStyle::Dark
    } else {
        UIUserInterfaceStyle::Light
    };
    let traits = UITraitCollection::traitCollectionWithUserInterfaceStyle(style);
    let color = UIColor::systemBackgroundColor().resolvedColorWithTraitCollection(&traits);
    view.setBackgroundColor(Some(&color));
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

/// Mirror `spec`'s toolbar onto `page` (module doc, Toolbar): its items with
/// flexible space between them and the primary trailing and prominent, each
/// tagged with its `chrome_actions` index; hidden, without animation, when
/// `spec` shows none.
fn apply_toolbar(
    mtm: MainThreadMarker,
    page: &Page,
    spec: &ChromeSpec,
    target: &FrustNativeControlTarget,
) {
    let Some(toolbar) = shown_toolbar(spec) else {
        page.page.setToolbarItems_animated(None, false);
        page.nav.setToolbarHidden_animated(true, false);
        return;
    };
    let offset = toolbar_offset(spec);
    let mut items = Vec::with_capacity(2 * toolbar.items.len() + 2);
    for (i, entry) in toolbar.items.iter().enumerate() {
        if i > 0 {
            items.push(UIBarButtonItem::flexibleSpaceItem(mtm));
        }
        items.push(action_item(mtm, offset + i, entry, target));
    }
    if let Some(primary) = &toolbar.primary {
        items.push(UIBarButtonItem::flexibleSpaceItem(mtm));
        let item = action_item(mtm, offset + toolbar.items.len(), primary, target);
        item.setStyle(UIBarButtonItemStyle::Prominent);
        items.push(item);
    }
    page.page
        .setToolbarItems_animated(Some(&NSArray::from_retained_slice(&items)), false);
    page.nav.setToolbarHidden_animated(false, false);
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

/// One top-bar or toolbar action, tagged with its `chrome_actions` index: a
/// pull-down when it has a menu, else a plain image (title as its label) or
/// title item reporting through `target`.
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
    // Action indices are small (the top bar's and toolbar's); the tag maps a tap back.
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

/// A tap on `controller`'s tab: deliver what it requests, then answer whether
/// UIKit may select it natively — YES for a foreign controller (the system
/// "More" list) and for another chrome page, which then already holds the
/// frust controller; NO for the shown page or while an apply has the chrome.
pub(super) fn tab_tapped(controller: &UIViewController) -> bool {
    let mut foreign = false;
    report("tab", |chrome| {
        let index = chrome.page_of(controller);
        foreign = index.is_none();
        index.map(ChromeTap::Tab)
    });
    if foreign {
        return true;
    }
    CHROME.with(|cell| {
        let Ok(mut state) = cell.try_borrow_mut() else {
            return false;
        };
        state
            .as_mut()
            .is_some_and(|chrome| chrome.native_select(controller))
    })
}

/// A tap on the top-bar or toolbar action item tagged `tag`.
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
        let event = resolve_tap(
            &tap_spec(&chrome.spec, chrome.pending.as_ref()),
            locate(chrome)?,
        )?;
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

        /// Pins the host view to the page's bounds, placing a held host that
        /// has no superview once this page is in the window.
        #[unsafe(method(viewDidLayoutSubviews))]
        fn view_did_layout_subviews(&self) {
            // SAFETY: forwarding the same no-argument selector to
            // `UIViewController`'s implementation, as UIKit expects.
            let _: () = unsafe { msg_send![super(self), viewDidLayoutSubviews] };
            if catch_unwind(AssertUnwindSafe(|| self.place_host())).is_err() {
                log::error!("frust-native-widgets: a chrome page layout step panicked");
            }
        }

        /// Places a held host as the page enters the window, before it shows
        /// (iOS 17+; earlier systems never send it, see the module doc).
        #[unsafe(method(viewIsAppearing:))]
        fn view_is_appearing(&self, animated: bool) {
            if UIViewController::class().responds_to(sel!(viewIsAppearing:)) {
                // SAFETY: forwarding the same selector with its one `BOOL`
                // argument to `UIViewController`'s implementation, checked
                // to exist, which UIKit requires an override to call.
                let _: () = unsafe { msg_send![super(self), viewIsAppearing: animated] };
            }
            if catch_unwind(AssertUnwindSafe(|| self.place_host())).is_err() {
                log::error!("frust-native-widgets: a chrome page appearance step panicked");
            }
        }

        /// Places a held host that still has no superview.
        #[unsafe(method(viewDidAppear:))]
        fn view_did_appear(&self, animated: bool) {
            // SAFETY: forwarding the same selector with its one `BOOL`
            // argument to `UIViewController`'s implementation, which UIKit
            // requires an override to call.
            let _: () = unsafe { msg_send![super(self), viewDidAppear: animated] };
            if catch_unwind(AssertUnwindSafe(|| self.place_host())).is_err() {
                log::error!("frust-native-widgets: a chrome page appearance step panicked");
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

    /// Contain `frust` and take charge of `host` (the wrapped `FrustView`),
    /// which [`Self::place_host`] moves into this view once it is in the window.
    fn hold(&self, frust: &UIViewController, host: &UIView) {
        self.addChildViewController(frust);
        frust.didMoveToParentViewController(Some(self));
        self.ivars().contained.replace(Some(frust.retain()));
        self.ivars().host.replace(Some(host.retain()));
    }

    /// Pin the held host to this view's bounds, first adding it here when
    /// this view is in the window, which drops the cover. Only the holding
    /// page has a host, so the host only ever lands in its parent's view.
    /// Whether the host is in this view afterwards.
    fn place_host(&self) -> bool {
        let host = self
            .ivars()
            .host
            .try_borrow()
            .ok()
            .and_then(|host| host.clone());
        let (Some(host), Some(view)) = (host, self.viewIfLoaded()) else {
            return false;
        };
        if is_superview(&host, &view) {
            host.setFrame(view.bounds());
            remove_cover(None);
            return true;
        }
        if view.window().is_none() {
            return false;
        }
        view.addSubview(&host);
        host.setFrame(view.bounds());
        remove_cover(None);
        true
    }

    /// Give up the frust controller, if held; the host view leaves this
    /// view too and has no superview until the next page places it.
    fn release(&self) {
        let Some(frust) = self.ivars().contained.take() else {
            return;
        };
        frust.willMoveToParentViewController(None);
        if let Some(host) = self.ivars().host.take()
            && let Some(view) = self.viewIfLoaded()
            && is_superview(&host, &view)
        {
            host.removeFromSuperview();
        }
        frust.removeFromParentViewController();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::ChromeEvent;

    const LEFT: usize = 0;
    const TAPPED: usize = 1;
    const THIRD: usize = 2;

    fn pending(deadline: Instant, tap: u64) -> NativeSelect {
        NativeSelect {
            from: LEFT,
            to: TAPPED,
            passed: Vec::new(),
            deadline,
            tap,
        }
    }

    fn three_tabs(selected: &str) -> ChromeSpec {
        ChromeSpec {
            tabs: ["left", "tapped", "third"]
                .into_iter()
                .map(|id| TabItem::new(id, id, TabIcon::AppleSymbol("circle".into())))
                .collect(),
            selected: Some(TabId::new(selected)),
            ..ChromeSpec::default()
        }
    }

    #[test]
    fn without_a_pending_tap_an_apply_follows_the_spec() {
        let now = Instant::now();
        assert_eq!(settle(None, Some(LEFT), now), Settle::Follow);
        assert_eq!(settle(None, None, now), Settle::Follow);
    }

    #[test]
    fn inside_the_window_a_spec_naming_the_left_page_is_stale() {
        let now = Instant::now();
        let tap = pending(now + SETTLE_WINDOW, 1);
        assert_eq!(settle(Some(&tap), Some(LEFT), now), Settle::Hold);
        assert_eq!(settle(Some(&tap), None, now), Settle::Hold);
    }

    #[test]
    fn the_tapped_page_confirms_and_a_third_page_overrides() {
        let now = Instant::now();
        let tap = pending(now + SETTLE_WINDOW, 1);
        assert_eq!(settle(Some(&tap), Some(TAPPED), now), Settle::Resolve);
        assert_eq!(settle(Some(&tap), Some(THIRD), now), Settle::Resolve);
    }

    #[test]
    fn past_the_deadline_an_apply_follows_the_spec_even_back() {
        let now = Instant::now();
        let tap = pending(now, 1);
        assert_eq!(settle(Some(&tap), Some(LEFT), now), Settle::Resolve);
        let later = now + Duration::from_millis(1);
        assert_eq!(settle(Some(&tap), Some(LEFT), later), Settle::Resolve);
    }

    #[test]
    fn an_unconfirmed_tap_snaps_back_at_its_deadline() {
        let tap = pending(Instant::now(), 3);
        assert_eq!(expire(Some(&tap), 3, Some(LEFT)), Expiry::SnapBack(LEFT));
    }

    #[test]
    fn a_tap_the_spec_confirms_or_names_no_page_settles_without_moving() {
        let tap = pending(Instant::now(), 3);
        assert_eq!(expire(Some(&tap), 3, Some(TAPPED)), Expiry::Settled);
        assert_eq!(expire(Some(&tap), 3, None), Expiry::Settled);
    }

    #[test]
    fn at_the_deadline_the_bar_moves_to_any_other_page_the_spec_names() {
        let tap = pending(Instant::now(), 3);
        assert_eq!(expire(Some(&tap), 3, Some(THIRD)), Expiry::SnapBack(THIRD));
    }

    #[test]
    fn a_stale_timer_never_touches_a_newer_or_settled_tap() {
        let newer = pending(Instant::now(), 4);
        assert_eq!(expire(Some(&newer), 3, Some(LEFT)), Expiry::Superseded);
        assert_eq!(expire(None, 3, Some(LEFT)), Expiry::Superseded);
    }

    #[test]
    fn a_first_tap_starts_from_the_shown_page_unless_it_is_no_chrome_page() {
        let now = Instant::now();
        let first = chain(None, LEFT, TAPPED, 3, now, 1);
        assert_eq!(first, Some(pending(now, 1)));
        assert_eq!(chain(None, 7, TAPPED, 3, now, 1), None);
    }

    #[test]
    fn a_second_tap_resolves_against_the_shown_tab_not_the_confirmed_one() {
        let spec = three_tabs("left");
        let first = pending(Instant::now() + SETTLE_WINDOW, 1);
        let shown = tap_spec(&spec, Some(&first));
        assert_eq!(
            resolve_tap(&shown, ChromeTap::Tab(LEFT)),
            Some(ChromeEvent::TabSelected(TabId::new("left")))
        );
        assert_eq!(
            resolve_tap(&shown, ChromeTap::Tab(TAPPED)),
            Some(ChromeEvent::TabReselected(TabId::new("tapped")))
        );
        assert_eq!(
            resolve_tap(&tap_spec(&spec, None), ChromeTap::Tab(LEFT)),
            Some(ChromeEvent::TabReselected(TabId::new("left")))
        );
    }

    #[test]
    fn a_double_tap_inside_the_window_keeps_the_latest_intent() {
        // Confirmed on LEFT; tap TAPPED, then tap LEFT again before any apply.
        let now = Instant::now();
        let deadline = now + SETTLE_WINDOW;
        let first = chain(None, LEFT, TAPPED, 3, deadline, 1).unwrap();
        let second = chain(Some(&first), TAPPED, LEFT, 3, deadline, 2).unwrap();
        assert_eq!((second.from, second.to), (LEFT, LEFT));
        assert_eq!(second.passed, vec![TAPPED]);
        // The app confirming the first tap is neither stale nor an override.
        assert_eq!(settle(Some(&second), Some(TAPPED), now), Settle::Hold);
        // Confirming the newest tap resolves.
        assert_eq!(settle(Some(&second), Some(LEFT), now), Settle::Resolve);
        // The bar already shows the newest tap's page: nothing to snap back.
        assert_eq!(expire(Some(&second), 2, Some(LEFT)), Expiry::Settled);
        // The first tap's timer is superseded by the second.
        assert_eq!(expire(Some(&second), 1, Some(LEFT)), Expiry::Superseded);
    }

    /// Confirmed on LEFT; tap TAPPED, then THIRD, inside the window.
    fn left_tapped_third(deadline: Instant) -> NativeSelect {
        let first = chain(None, LEFT, TAPPED, 3, deadline, 1).unwrap();
        chain(Some(&first), TAPPED, THIRD, 3, deadline, 2).unwrap()
    }

    #[test]
    fn a_chained_tap_onto_a_third_page_converges_to_the_confirmed_page() {
        let now = Instant::now();
        let second = left_tapped_third(now + SETTLE_WINDOW);
        assert_eq!((second.from, second.to), (LEFT, THIRD));
        assert_eq!(second.passed, vec![TAPPED]);
        // Inside the window an intermediate or stale confirmation keeps the bar.
        assert_eq!(settle(Some(&second), Some(LEFT), now), Settle::Hold);
        assert_eq!(settle(Some(&second), Some(TAPPED), now), Settle::Hold);
        assert_eq!(settle(Some(&second), Some(THIRD), now), Settle::Resolve);
        // At the deadline the bar moves to whichever page the spec names.
        assert_eq!(expire(Some(&second), 2, Some(LEFT)), Expiry::SnapBack(LEFT));
        assert_eq!(
            expire(Some(&second), 2, Some(TAPPED)),
            Expiry::SnapBack(TAPPED)
        );
    }

    #[test]
    fn left_tapped_third_confirmed_only_up_to_tapped_snaps_to_tapped() {
        // The app accepts TabSelected(TAPPED) and rejects TabSelected(THIRD).
        let now = Instant::now();
        let second = left_tapped_third(now + SETTLE_WINDOW);
        assert_eq!(settle(Some(&second), Some(TAPPED), now), Settle::Hold);
        assert_eq!(
            expire(Some(&second), 2, Some(TAPPED)),
            Expiry::SnapBack(TAPPED)
        );
    }

    #[test]
    fn left_tapped_left_with_the_second_tap_ignored_snaps_to_tapped() {
        // The app accepts TabSelected(TAPPED) and ignores the TabSelected(LEFT).
        let now = Instant::now();
        let deadline = now + SETTLE_WINDOW;
        let first = chain(None, LEFT, TAPPED, 3, deadline, 1).unwrap();
        let second = chain(Some(&first), TAPPED, LEFT, 3, deadline, 2).unwrap();
        assert_eq!(settle(Some(&second), Some(TAPPED), now), Settle::Hold);
        assert_eq!(
            expire(Some(&second), 2, Some(TAPPED)),
            Expiry::SnapBack(TAPPED)
        );
    }

    #[test]
    fn left_tapped_third_confirmed_third_settles() {
        let now = Instant::now();
        let second = left_tapped_third(now + SETTLE_WINDOW);
        assert_eq!(expire(Some(&second), 2, Some(THIRD)), Expiry::Settled);
    }
}
