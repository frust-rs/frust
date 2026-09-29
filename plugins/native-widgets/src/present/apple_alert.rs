//! The iOS/iPadOS alert arm: a `UIAlertController` built, presented and
//! resolved on the main thread, over `super::apple_host`'s shared helpers
//! (the main-queue hop, host discovery and the live-presentation guard).
//!
//! # Mapping
//!
//! | [`AlertSpec`] | UIKit |
//! |---|---|
//! | `style: Alert` / `ActionSheet` | `UIAlertControllerStyleAlert` / `…ActionSheet` |
//! | `role: Default` / `Cancel` / `Destructive` | `UIAlertActionStyleDefault` / `…Cancel` / `…Destructive` |
//! | an empty `title` / `message` | `nil` (UIKit omits the line rather than reserving it) |
//! | `anchor` (action sheet) | the popover's `sourceView` (the presenting controller's view) + `sourceRect` (the anchor converted from window coordinates) |
//! | `cancelable` | an iPad popover's outside tap (`presentationControllerShouldDismiss:`) |
//!
//! UIKit places a `Cancel`-role action itself (last on an alert, set apart on
//! an action sheet), whatever its position in the spec.
//!
//! # `cancelable`
//!
//! A UIKit alert — and an iPhone action sheet — has no outside-tap
//! dismissal at all, so `cancelable: false` removes nothing there: the user
//! still leaves only through an action. The one dismissal UIKit offers
//! without an action is an **iPad action sheet's popover** outside tap,
//! which `cancelable` gates (the delegate's `presentationControllerShouldDismiss:`
//! answers it). UIKit hides an action sheet's `Cancel`-role action inside a
//! popover and runs *its* handler on that outside tap, so a cancelable iPad
//! sheet with a `Cancel` action resolves [`AlertOutcome::Action`] with that
//! action's id; without one it resolves [`AlertOutcome::Cancelled`].
//!
//! # The iPad anchor rule
//!
//! UIKit raises (`NSGenericException`, a crash) when an action sheet is
//! presented as a popover with no source, so an [`AlertStyle::ActionSheet`]
//! **without** [`AlertSpec::anchor`] on an iPad (`UIDevice.userInterfaceIdiom
//! == .pad`) resolves [`PresentError::InvalidSpec`] before anything is
//! built — synchronously when requested on the main thread, else from the
//! main-queue step. An iPhone presents an unanchored sheet from the bottom
//! edge, so the rule is iPad-only; an anchor given on an iPhone is ignored.
//! As a backstop, whenever UIKit hands back a popover controller for the
//! sheet and there is no anchor to point it at, the presenting step
//! resolves the same `InvalidSpec` instead of presenting — never a crash.
//!
//! # Resolution
//!
//! Every path resolves the presentation by first taking its live entry back
//! **by generation** (`apple_host::take_live_as`) — the first path to get
//! there wins, and every later one (a second handler, a delegate callback
//! after an action, a stale notification) finds nothing:
//!
//! - **An action** — one `RcBlock` handler per action, capturing the
//!   generation and the action's id → [`AlertOutcome::Action`]. UIKit runs a
//!   handler after the alert's dismissal animation, so the next presentation
//!   may start at once.
//! - **An iPad popover outside tap** — the delegate's
//!   `presentationControllerDidDismiss:` → [`AlertOutcome::Cancelled`].
//! - **[`super::dismiss`]** — `dismissViewControllerAnimated:completion:`,
//!   resolving [`AlertOutcome::Dismissed`] from the completion (so the next
//!   presentation never races a dismissal still animating). A dismiss that
//!   arrives while UIKit is already dismissing the alert for a user's tap
//!   leaves it to the tap, whose handler resolves it.
//! - **The scene going away** — the delegate observes
//!   `UISceneDidDisconnectNotification` for the presenting window's scene →
//!   [`AlertOutcome::HostLost`].
//! - **UIKit refusing to present** (the anchor controller left the window
//!   hierarchy between discovery and presentation) → [`PresentError::NoHost`].
//!
//! # One delegate object per presentation
//!
//! [`FrustNativeAlertDelegate`] is a `define_class!` object carrying its
//! presentation's generation. It is the popover's delegate (a weak UIKit
//! property) and the scene-disconnect observer, and is held strongly by the
//! live entry for the presentation's lifetime; it holds nothing itself, so
//! there is no retain cycle. When the entry is resolved, the controller and
//! the delegate are **autoreleased** rather than released, because the
//! resolving call is usually running *inside* one of them (a handler the
//! controller invokes, a delegate method) and must not free its own caller.
//!
//! # A displaced presentation
//!
//! A caller that drops its [`super::Presentation`] frees the Busy slot but
//! not the presentation on screen, so a newer request (of either kind) can
//! find an older alert or sheet still live. Whatever its kind, it goes down
//! through the shared [`super::apple_host::LivePresentation::dismiss`]
//! seam — its own `Dismissed`/`Dismissed(Programmatic)` outcome goes to the
//! dropped receiver and is discarded — and this alert presents from that
//! dismissal's completion, from a freshly discovered anchor. A displaced
//! entry already mid-dismissal for some other reason resolves and hands
//! over from that same completion, never re-parked under its own
//! generation ([`super::apple_host::displaced_action`]).
//!
//! # Not observed
//!
//! The app itself tearing down the controller the alert is presented from
//! (dismissing a modal that hosts the alert, replacing the window's root
//! controller) makes UIKit remove the alert with no callback this arm can
//! see without subclassing `UIAlertController` (which Apple forbids). The
//! presentation then stays pending until [`super::dismiss`] resolves it
//! `Dismissed` or its caller drops the future.
//!
//! # `unsafe`
//!
//! Four blocks, each with its own `SAFETY` comment: `NSObject`'s `init` on
//! the delegate, the popover's weak `setDelegate:`, reading the
//! `UISceneDidDisconnectNotification` extern static, and the selector-based
//! `addObserver:selector:name:object:`/`removeObserver:` pair.

#[cfg(not(target_os = "ios"))]
compile_error!("the UIKit alert arm targets iOS only");

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSString};
use objc2_ui_kit::{
    UIAdaptivePresentationControllerDelegate, UIAlertAction, UIAlertActionStyle, UIAlertController,
    UIAlertControllerStyle, UIDevice, UIPopoverPresentationControllerDelegate,
    UIPresentationController, UISceneDidDisconnectNotification, UIUserInterfaceIdiom,
};

use super::apple_host::{
    AfterDismiss, DisplacedAction, LivePresentation, dismiss_live, displaced_action, install_live,
    on_main, presenting_anchor, take_live_as,
};
use super::{
    ActionRole, AlertHost, AlertOutcome, AlertSpec, AlertStyle, AnchorRect, PresentError, Sender,
};

/// An action's handler block — UIKit's `void (^)(UIAlertAction *)`.
type ActionHandler = RcBlock<dyn Fn(NonNull<UIAlertAction>)>;

/// The iOS host: `super::AlertHost` over `UIAlertController`.
pub(crate) struct Host;

impl AlertHost for Host {
    fn show_alert(
        spec: AlertSpec,
        tx: Sender<AlertOutcome>,
        generation: u64,
    ) -> Result<(), PresentError> {
        // A caller already on the main thread (a `spawn_local` task, a
        // control's callback) hears the iPad anchor rule synchronously.
        if let Some(mtm) = MainThreadMarker::new() {
            check_anchor_rule(mtm, &spec)?;
        }
        on_main(move |mtm| start(mtm, spec, tx, generation));
        Ok(())
    }

    fn dismiss(generation: u64) {
        dismiss_live(generation);
    }
}

/// The iPad anchor rule — see the module doc.
fn check_anchor_rule(mtm: MainThreadMarker, spec: &AlertSpec) -> Result<(), PresentError> {
    if spec.style == AlertStyle::ActionSheet
        && spec.anchor.is_none()
        && UIDevice::currentDevice(mtm).userInterfaceIdiom() == UIUserInterfaceIdiom::Pad
    {
        return Err(PresentError::InvalidSpec(
            "an action sheet on iPad needs an anchor (AlertSpec::anchor): UIKit presents it as \
             a popover, which must point at something"
                .to_string(),
        ));
    }
    Ok(())
}

/// The main-queue half of a request: build the alert, park it as the live
/// entry, then present it (after taking down a displaced one, if any).
fn start(mtm: MainThreadMarker, spec: AlertSpec, tx: Sender<AlertOutcome>, generation: u64) {
    if let Err(err) = check_anchor_rule(mtm, &spec) {
        tx.send(Err(err));
        return;
    }
    let anchor = spec.anchor;
    let live = LiveAlert::build(mtm, &spec, tx, generation);
    match install_live(mtm, generation, Box::new(live)) {
        None => present_live(mtm, generation, anchor),
        Some(displaced) => {
            let then: AfterDismiss = Box::new(move |mtm| present_live(mtm, generation, anchor));
            // Whatever kind `displaced` is, its own `dismiss` takes it down
            // and runs `then` from that dismissal's completion (module
            // doc's *A displaced presentation*).
            displaced.dismiss(mtm, Some(then));
        }
    }
}

/// Present the live entry for `generation` from a freshly discovered anchor.
/// A no-op when the entry is gone (resolved while a displaced alert was
/// being taken down).
fn present_live(mtm: MainThreadMarker, generation: u64, anchor: Option<AnchorRect>) {
    let Some(live) = take_live_as::<LiveAlert>(mtm, generation) else {
        return;
    };
    let Some(presenter) = presenting_anchor(mtm) else {
        live.finish(Err(PresentError::NoHost));
        return;
    };

    if let Some(popover) = live.controller.popoverPresentationController() {
        // SAFETY: `delegate` is a weak property; the live entry holds the
        // delegate strongly until the presentation resolves, and `finish`
        // autoreleases it, so UIKit never messages a freed delegate.
        unsafe {
            popover.setDelegate(Some(ProtocolObject::from_ref(&*live.delegate)));
        }
        let Some((rect, view)) = anchor.zip(presenter.view()) else {
            // UIKit raises on a popover with no source. The idiom check
            // catches this on an iPad; this also covers any other idiom
            // UIKit presents an action sheet as a popover on.
            live.finish(Err(PresentError::InvalidSpec(
                "this device presents the action sheet as a popover, which needs an anchor \
                 (AlertSpec::anchor)"
                    .to_string(),
            )));
            return;
        };
        popover.setSourceView(Some(&view));
        popover.setSourceRect(view.convertRect_fromView(cg_rect(rect), None));
    }

    presenter.presentViewController_animated_completion(&live.controller, true, None);
    if live.controller.presentingViewController().is_none() {
        log::warn!(
            "frust-native-widgets: UIKit refused to present the alert (the presenting controller \
             is not in a window hierarchy)"
        );
        live.finish(Err(PresentError::NoHost));
        return;
    }

    let scene = presenter
        .view()
        .and_then(|view| view.window())
        .and_then(|window| window.windowScene());
    if let Some(scene) = scene {
        live.delegate.observe_disconnect(&scene);
    } else {
        log::debug!(
            "frust-native-widgets: the presenting controller has no window scene; the alert's \
             HostLost is not observed"
        );
    }
    // Re-parked under the same generation, which nothing else can have
    // claimed during this main-queue turn.
    if let Some(unexpected) = install_live(mtm, generation, live) {
        unexpected.dismiss(mtm, None);
    }
}

fn cg_rect(rect: AnchorRect) -> CGRect {
    CGRect::new(
        CGPoint::new(rect.x, rect.y),
        CGSize::new(rect.width, rect.height),
    )
}

/// Run a UIKit callback's body without letting a panic unwind into UIKit's
/// frames (`docs/PLUGINS_CODE_STANDARDS.md`'s no-unwind-near-FFI rule). The
/// sender a panicking body held is dropped with it, which resolves its
/// presentation as a platform error rather than leaving it pending.
fn guarded(which: &str, body: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(body)).is_err() {
        log::error!("frust-native-widgets: the alert's {which} callback panicked");
    }
}

/// Resolve presentation `generation` with `outcome` if it is still the live
/// entry — the one path every UIKit callback takes.
fn resolve(generation: u64, outcome: Result<AlertOutcome, PresentError>) {
    // Every UIKit callback this arm registers runs on the main thread; a
    // `None` here would be a UIKit contract break, answered by doing nothing.
    let Some(mtm) = MainThreadMarker::new() else {
        log::error!("frust-native-widgets: an alert callback arrived off the main thread");
        return;
    };
    if let Some(live) = take_live_as::<LiveAlert>(mtm, generation) {
        live.finish(outcome);
    }
}

/// Everything one presented alert keeps alive until its outcome.
struct LiveAlert {
    generation: u64,
    tx: Sender<AlertOutcome>,
    controller: Retained<UIAlertController>,
    delegate: Retained<FrustNativeAlertDelegate>,
    /// Retained by their `UIAlertAction`s too; held here so the entry owns
    /// everything the presentation built.
    _handlers: Vec<ActionHandler>,
}

impl LiveAlert {
    fn build(
        mtm: MainThreadMarker,
        spec: &AlertSpec,
        tx: Sender<AlertOutcome>,
        generation: u64,
    ) -> Self {
        let title = non_empty(&spec.title);
        let message = non_empty(&spec.message);
        let style = match spec.style {
            AlertStyle::Alert => UIAlertControllerStyle::Alert,
            AlertStyle::ActionSheet => UIAlertControllerStyle::ActionSheet,
        };
        let controller = UIAlertController::alertControllerWithTitle_message_preferredStyle(
            title.as_deref(),
            message.as_deref(),
            style,
            mtm,
        );

        let mut handlers = Vec::with_capacity(spec.actions.len());
        for action in &spec.actions {
            let id = action.id.clone();
            let handler: ActionHandler = RcBlock::new(move |_: NonNull<UIAlertAction>| {
                guarded("action", || {
                    resolve(generation, Ok(AlertOutcome::Action(id.clone())));
                });
            });
            let ui_action = UIAlertAction::actionWithTitle_style_handler(
                Some(&NSString::from_str(&action.label)),
                action_style(action.role),
                Some(&handler),
                mtm,
            );
            controller.addAction(&ui_action);
            handlers.push(handler);
        }

        Self {
            generation,
            tx,
            controller,
            delegate: FrustNativeAlertDelegate::new(mtm, generation, spec.cancelable),
            _handlers: handlers,
        }
    }

    /// The terminal step: stop observing, deliver `outcome`, and let the
    /// UIKit objects go at the end of the current run-loop turn (module
    /// doc's *One delegate object per presentation*).
    fn finish(self, outcome: Result<AlertOutcome, PresentError>) {
        let Self {
            tx,
            controller,
            delegate,
            _handlers,
            ..
        } = self;
        delegate.stop_observing();
        tx.send(outcome);
        let _ = Retained::autorelease_ptr(controller);
        let _ = Retained::autorelease_ptr(delegate);
    }

    /// Take the alert down programmatically, resolving `Dismissed` once
    /// UIKit's dismissal completes, then run `then` (a displaced alert's
    /// successor) — [`displaced_action`] decides which once the alert is
    /// confirmed on screen; not on screen at all always resolves and hands
    /// off at once, with nothing to wait for.
    fn take_down(self: Box<Self>, mtm: MainThreadMarker, then: Option<AfterDismiss>) {
        let Some(presenter) = self.controller.presentingViewController() else {
            // Not on screen (never presented, or removed without a
            // callback): nothing to wait for.
            self.finish(Ok(AlertOutcome::Dismissed));
            if let Some(then) = then {
                then(mtm);
            }
            return;
        };

        match displaced_action(self.controller.isBeingDismissed(), then.is_some()) {
            DisplacedAction::Repark => {
                // UIKit is already dismissing it for a user's tap, whose
                // handler resolves it next; a programmatic dismiss loses
                // that race.
                let generation = self.generation;
                if let Some(unexpected) = install_live(mtm, generation, self) {
                    unexpected.dismiss(mtm, None);
                }
            }
            DisplacedAction::ResolveAndContinue => {
                // A displaced alert already on its way out: a second
                // dismissal would be ignored (and its completion never
                // run), so resolve it now and let the successor present —
                // or resolve `NoHost` if UIKit refuses mid-transition.
                self.finish(Ok(AlertOutcome::Dismissed));
                if let Some(then) = then {
                    then(mtm);
                }
            }
            DisplacedAction::CloseThenContinue => {
                let pending = Cell::new(Some((self, then)));
                let completion = RcBlock::new(move || {
                    guarded("dismissal completion", || {
                        let Some((live, then)) = pending.take() else {
                            return;
                        };
                        live.finish(Ok(AlertOutcome::Dismissed));
                        if let (Some(then), Some(mtm)) = (then, MainThreadMarker::new()) {
                            then(mtm);
                        }
                    });
                });
                presenter.dismissViewControllerAnimated_completion(true, Some(&completion));
            }
        }
    }
}

impl LivePresentation for LiveAlert {
    fn dismiss(self: Box<Self>, mtm: MainThreadMarker, then: Option<AfterDismiss>) {
        self.take_down(mtm, then);
    }
}

fn non_empty(text: &str) -> Option<Retained<NSString>> {
    (!text.is_empty()).then(|| NSString::from_str(text))
}

fn action_style(role: ActionRole) -> UIAlertActionStyle {
    match role {
        ActionRole::Default => UIAlertActionStyle::Default,
        ActionRole::Cancel => UIAlertActionStyle::Cancel,
        ActionRole::Destructive => UIAlertActionStyle::Destructive,
    }
}

/// The per-presentation delegate's state: whose presentation it answers
/// for, and whether an iPad popover may be dismissed by an outside tap.
struct DelegateIvars {
    generation: u64,
    cancelable: bool,
}

define_class!(
    // SAFETY:
    // - `NSObject` has no subclassing requirements.
    // - The ivars are two plain values with no `Drop` impl, so the macro's
    //   generated `dealloc` has nothing extra to uphold.
    #[unsafe(super(NSObject))]
    // Every UIKit presentation callback runs on the main thread, and the
    // arm builds it there (`Self::alloc(mtm)`).
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateIvars]
    struct FrustNativeAlertDelegate;

    unsafe impl NSObjectProtocol for FrustNativeAlertDelegate {}

    unsafe impl UIAdaptivePresentationControllerDelegate for FrustNativeAlertDelegate {
        /// An iPad popover's outside tap: allowed only for a cancelable spec.
        #[unsafe(method(presentationControllerShouldDismiss:))]
        fn presentation_controller_should_dismiss(
            &self,
            _controller: &UIPresentationController,
        ) -> bool {
            self.ivars().cancelable
        }

        /// The popover went away through the user (an outside tap) →
        /// `Cancelled`, unless a `Cancel`-role action's handler got there
        /// first (module doc's *`cancelable`*).
        #[unsafe(method(presentationControllerDidDismiss:))]
        fn presentation_controller_did_dismiss(&self, _controller: &UIPresentationController) {
            let generation = self.ivars().generation;
            guarded("popover dismissal", || {
                resolve(generation, Ok(AlertOutcome::Cancelled));
            });
        }
    }

    unsafe impl UIPopoverPresentationControllerDelegate for FrustNativeAlertDelegate {}

    impl FrustNativeAlertDelegate {
        /// `UISceneDidDisconnectNotification` for the presenting scene →
        /// `HostLost`.
        #[unsafe(method(sceneDidDisconnect:))]
        fn scene_did_disconnect(&self, _notification: &NSNotification) {
            let generation = self.ivars().generation;
            guarded("scene disconnect", || {
                resolve(generation, Ok(AlertOutcome::HostLost));
            });
        }
    }
);

impl FrustNativeAlertDelegate {
    fn new(mtm: MainThreadMarker, generation: u64, cancelable: bool) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars {
            generation,
            cancelable,
        });
        // SAFETY: `NSObject`'s designated initializer, called on a freshly
        // allocated instance whose ivars are already set (the idiom
        // `crate::apple::events`' target class uses).
        unsafe { msg_send![super(this), init] }
    }

    /// Observe the presenting scene's disconnection (→ `HostLost`).
    fn observe_disconnect(&self, scene: &AnyObject) {
        let center = NSNotificationCenter::defaultCenter();
        // SAFETY: `UISceneDidDisconnectNotification` is a UIKit constant
        // `NSString`, initialized before any Rust code runs.
        let name = unsafe { UISceneDidDisconnectNotification };
        // SAFETY: `self` is a live object implementing `sceneDidDisconnect:`
        // (defined above, taking the one `NSNotification` argument the
        // selector-based API passes), `name` is a notification name and
        // `scene` the object posting it. `stop_observing` removes this
        // observer before the live entry lets the delegate go.
        unsafe {
            center.addObserver_selector_name_object(
                self,
                sel!(sceneDidDisconnect:),
                Some(name),
                Some(scene),
            );
        }
    }

    /// Undo [`Self::observe_disconnect`] — a no-op when it never ran.
    fn stop_observing(&self) {
        // SAFETY: removing an observer, registered or not, has no
        // precondition beyond `self` being a live object.
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(self) };
    }
}
