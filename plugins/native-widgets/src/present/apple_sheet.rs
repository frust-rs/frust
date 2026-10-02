//! The iOS/iPadOS sheet arm: a plugin-owned `UIViewController` subclass
//! ([`FrustNativeSheetController`]) whose view is a `UIStackView` of native
//! rows, presented as a page sheet under `UISheetPresentationController` —
//! built, presented and resolved on the main thread over
//! `super::apple_host`'s shared helpers (the main-queue hop, host discovery
//! and the live-presentation guard), exactly like the alert arm
//! (`super::apple_alert`).
//!
//! # Mapping
//!
//! | [`SheetSpec`] | UIKit |
//! |---|---|
//! | `content.title` | a `UILabel`, bold system 22 pt, wrapping |
//! | `content.message` | a `UILabel`, system 17 pt, `secondaryLabelColor`, wrapping |
//! | `content.image` | a `UIImageView` (`imageWithData:`), aspect-fit, at most [`IMAGE_MAX_HEIGHT`] tall; undecodable bytes omit the row (logged) |
//! | `content.actions[i]` | a system `UIButton` tagged `i`, target-action on the controller; `Default` wears the tint, `Destructive` `systemRedColor`, `Cancel` `secondaryLabelColor` |
//! | `detents` | `sheetPresentationController.detents`: `mediumDetent` / `largeDetent` / `customDetentWithIdentifier:resolver:` (fraction × `maximumDetentValue`) |
//! | `selected` | `selectedDetentIdentifier` |
//! | `grabber` | `prefersGrabberVisible` |
//! | `scrolling_expands` | `prefersScrollingExpandsWhenScrolledToEdge` |
//! | `largest_undimmed` | `largestUndimmedDetentIdentifier` |
//! | `corner_radius` | `preferredCornerRadius` |
//! | `dismissible` | `presentationControllerShouldDismiss:` |
//! | `tint` / `dark` | the root view's `tintColor` / the controller's `overrideUserInterfaceStyle` |
//! | (always) | `modalPresentationStyle = .pageSheet`, `prefersEdgeAttachedInCompactHeight = true`, `widthFollowsPreferredContentSizeWhenEdgeAttached = true` |
//!
//! The rows stack top to bottom inside the safe area, top-aligned (the stack
//! is pinned at the top and leading/trailing edges, and bounded — not pinned
//! — at the bottom), so a medium detent shows the head of the content and a
//! large one the rest. No `platform_view` slot is ever mounted inside the
//! sheet (`docs/CODE_STANDARDS.md`'s Platform-View Conventions: a presented
//! controller is not a slot), and no frust content either — frust has one
//! render root.
//!
//! # Availability, checked at runtime
//!
//! The crate's floor is iOS 15, which has `UISheetPresentationController`,
//! medium/large detents, the grabber and `largestUndimmedDetentIdentifier`.
//! Custom detents are iOS 16+, and the bindings carry no availability `cfg`,
//! so [`custom_detents_available`] asks the runtime
//! (`UISheetPresentationControllerDetent` answering
//! `customDetentWithIdentifier:resolver:`) before building one. Below 16 a
//! [`Detent::Custom`] becomes the nearest of medium (`f <= 0.75`) or large
//! ([`Detent::system_fallback`]), duplicates collapse, and one warning is logged per
//! process.
//!
//! # iPad in regular width ignores detents
//!
//! UIKit presents a page sheet in a regular-width iPad window as a centered
//! form sheet at a fixed size; only an edge-attached sheet (compact width,
//! or compact height with `prefersEdgeAttachedInCompactHeight`) rests at its
//! detents. The arm configures the detents anyway (they take effect the
//! moment the window turns compact — Slide Over, Split View) and never
//! fakes parity — `docs/LIMITATIONS.md`'s
//! `native-sheet-ipad-regular-width-detents`.
//!
//! # Resolution
//!
//! Every path resolves the presentation by first taking its live entry back
//! **by generation** (`apple_host::take_live_as`) — the first path to get
//! there wins, and every later one finds nothing:
//!
//! - **An action row** — the controller's `frustSheetAction:` reads the
//!   button's tag → the sheet dismisses itself
//!   (`dismissViewControllerAnimated:completion:`) and resolves
//!   [`SheetOutcome::Action`] from the completion, so the caller's
//!   continuation never races a sheet still sliding away.
//! - **A swipe-down** (only when `presentationControllerShouldDismiss:`
//!   answered `dismissible`) — `presentationControllerDidDismiss:` →
//!   [`SheetOutcome::Dismissed`]`(`[`DismissReason::User`]`)`.
//! - **[`super::SheetHandle::dismiss`] / [`super::dismiss`]** — the same
//!   self-dismissal, resolving `Dismissed(Programmatic)` from its
//!   completion. A dismiss arriving while UIKit is already dismissing the
//!   sheet for the user (an interactive swipe in progress) records itself
//!   as pending instead of racing it: if the swipe completes,
//!   `presentationControllerDidDismiss:` resolves it as the user's; if the
//!   swipe is cancelled, `viewDidAppear:` (which UIKit calls again once the
//!   sheet is fully back) retries the dismiss, so the request is never
//!   silently dropped.
//! - **The host going away** — the scene disconnecting
//!   (`UISceneDidDisconnectNotification` for the presenting window's scene),
//!   or the sheet disappearing with no presenter left (the app tore down the
//!   controller it was presented from: `viewDidDisappear:`, checked one
//!   main-queue turn later so a swipe-down's own callback wins) →
//!   [`SheetOutcome::HostLost`]. A sheet merely covered by another
//!   presentation still has its presenter and stays live.
//! - **UIKit refusing to present** (the anchor controller left the window
//!   hierarchy between discovery and presentation) → [`PresentError::NoHost`].
//!
//! Detent changes are not outcomes: `sheetPresentationControllerDidChangeSelectedDetentIdentifier:`
//! maps the identifier back through the controller's own table and calls
//! [`SheetSpec::on_detent`]'s listener, leaving the entry live.
//! [`super::SheetHandle::select_detent`] animates `selectedDetentIdentifier`
//! (`animateChanges:`) and, following UIKit, is not echoed back.
//!
//! # One object per presentation
//!
//! The alert arm needs a separate delegate object because Apple forbids
//! subclassing `UIAlertController`. A sheet presents a controller this crate
//! owns, so the one `define_class!` object is the whole family: the
//! presented controller, its sheet presentation controller's delegate
//! (adaptive + sheet callbacks, a weak UIKit property), every action
//! button's target (weak too) and the scene-disconnect observer. It carries
//! its generation, its action ids, its detent table and the detent listener
//! as ivars; the live entry ([`LiveSheet`]) holds it strongly with the
//! sender and the custom-detent resolver blocks. Nothing it holds points
//! back at it, so there is no retain cycle. On resolution the controller is
//! **autoreleased**, not released: the resolving call usually runs inside
//! one of its own methods.
//!
//! # A displaced presentation
//!
//! As in the alert arm: a caller dropping its future frees the Busy slot,
//! not the presentation, so a newer request (of either kind) can find an
//! older alert or sheet still live. Whatever its kind, it goes down through
//! the shared [`super::apple_host::LivePresentation::dismiss`] seam and this
//! sheet presents from that dismissal's completion, from a freshly
//! discovered anchor — a displaced entry already mid-dismissal for some
//! other reason resolves and hands over from that same completion, never
//! re-parked under its own generation ([`super::apple_host::displaced_action`]).
//!
//! # `unsafe`
//!
//! Each site carries its own `SAFETY` comment: the controller's `init` and
//! its `viewDidDisappear:`/`viewDidAppear:` super calls; the sheet
//! presentation controller's weak `setDelegate:`; each button's weak
//! `addTarget:action:forControlEvents:`; `UIView.setTintColor:` (an
//! unannotated-nullability setter); a custom detent resolver's borrow of its
//! context pointer; reading UIKit's extern constants (the medium/large
//! detent identifiers, `UISheetPresentationControllerDetentInactive`,
//! `UISceneDidDisconnectNotification`); and the selector-based
//! `addObserver:selector:name:object:` / `removeObserver:` pair.

#[cfg(not(target_os = "ios"))]
compile_error!("the UIKit sheet arm targets iOS only");

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_core_foundation::CGFloat;
use objc2_foundation::{NSArray, NSData, NSNotification, NSNotificationCenter, NSString};
use objc2_ui_kit::{
    NSLayoutConstraint, UIAdaptivePresentationControllerDelegate, UIButton, UIButtonType, UIColor,
    UIControlEvents, UIControlState, UIFont, UIImage, UIImageView, UILabel, UILayoutConstraintAxis,
    UIModalPresentationStyle, UIPresentationController, UIResponder,
    UISceneDidDisconnectNotification, UISheetPresentationController,
    UISheetPresentationControllerDelegate, UISheetPresentationControllerDetent,
    UISheetPresentationControllerDetentIdentifierLarge,
    UISheetPresentationControllerDetentIdentifierMedium,
    UISheetPresentationControllerDetentInactive,
    UISheetPresentationControllerDetentResolutionContext, UIStackView, UIStackViewAlignment,
    UIUserInterfaceStyle, UIView, UIViewContentMode, UIViewController,
};

use super::apple_host::{
    AfterDismiss, DisplacedAction, LivePresentation, dismiss_live, displaced_action, install_live,
    on_main, presenting_anchor, take_live_as,
};
use super::{
    ActionRole, Detent, DismissReason, PresentError, Sender, SheetHost, SheetOutcome, SheetSpec,
};
use crate::controls::platform::{set_label_font, set_label_text_color, ui_color};

/// The tallest an image row may grow, in points — a sheet's image is an
/// illustration, not a viewer.
const IMAGE_MAX_HEIGHT: CGFloat = 200.0;
/// Vertical space between rows, in points.
const ROW_SPACING: CGFloat = 12.0;
/// The content's inset from the safe area: top (clearing the grabber),
/// sides, and the minimum bottom margin.
const TOP_INSET: CGFloat = 28.0;
const SIDE_INSET: CGFloat = 20.0;
const BOTTOM_INSET: CGFloat = 20.0;
/// An action row's minimum height — Apple's 44 pt hit target.
const ACTION_MIN_HEIGHT: CGFloat = 44.0;

/// A custom detent's resolver block — UIKit's
/// `CGFloat (^)(id<UISheetPresentationControllerDetentResolutionContext>)`.
type DetentResolver = RcBlock<
    dyn Fn(
        NonNull<ProtocolObject<dyn UISheetPresentationControllerDetentResolutionContext>>,
    ) -> CGFloat,
>;

/// The iOS host: `super::SheetHost` over `UISheetPresentationController`.
pub(crate) struct Host;

impl SheetHost for Host {
    fn show_sheet(
        spec: SheetSpec,
        tx: Sender<SheetOutcome>,
        generation: u64,
    ) -> Result<(), PresentError> {
        on_main(move |mtm| start(mtm, spec, tx, generation));
        Ok(())
    }

    fn dismiss(generation: u64) {
        dismiss_live(generation);
    }

    fn select_detent(generation: u64, detent: Detent) {
        on_main(move |mtm| {
            if let Some(live) = take_live_as::<LiveSheet>(mtm, generation) {
                live.select(detent);
                park(mtm, generation, live);
            }
        });
    }
}

/// Whether this OS builds custom detents (iOS 16+) — see the module doc's
/// *Availability*. `customDetentWithIdentifier:resolver:` is a class
/// method, so the question goes to the metaclass.
fn custom_detents_available() -> bool {
    UISheetPresentationControllerDetent::class()
        .metaclass()
        .responds_to(sel!(customDetentWithIdentifier:resolver:))
}

/// `detent` as this OS will present it: itself where custom detents exist,
/// else its [`Detent::system_fallback`] (warning once per process).
fn effective_detent(detent: Detent, custom_available: bool) -> Detent {
    if custom_available || !matches!(detent, Detent::Custom(_)) {
        return detent;
    }
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        log::warn!(
            "frust-native-widgets: custom sheet detents need iOS 16; substituting the nearest of \
             medium/large on this OS"
        );
    }
    detent.system_fallback()
}

/// The identifier UIKit knows `detent` by: the system constants for
/// medium/large, a stable, fraction-derived name for a custom one.
fn identifier(detent: Detent) -> Retained<NSString> {
    match detent {
        // SAFETY: both are UIKit constant `NSString`s, initialized before
        // any Rust code runs (the same claim the alert arm makes for
        // `UISceneDidDisconnectNotification`).
        Detent::Medium => unsafe { UISheetPresentationControllerDetentIdentifierMedium }.retain(),
        // SAFETY: as above.
        Detent::Large => unsafe { UISheetPresentationControllerDetentIdentifierLarge }.retain(),
        Detent::Custom(fraction) => NSString::from_str(&format!(
            "dev.frust.nativewidgets.sheet.custom.{:016x}",
            fraction.to_bits()
        )),
    }
}

/// Build the UIKit detent for `detent` (already effective), keeping a custom
/// detent's resolver block in `resolvers`.
fn detent_object(
    mtm: MainThreadMarker,
    detent: Detent,
    resolvers: &mut Vec<DetentResolver>,
) -> Retained<UISheetPresentationControllerDetent> {
    match detent {
        Detent::Medium => UISheetPresentationControllerDetent::mediumDetent(mtm),
        Detent::Large => UISheetPresentationControllerDetent::largeDetent(mtm),
        Detent::Custom(fraction) => {
            let resolver: DetentResolver = RcBlock::new(
                move |context: NonNull<
                    ProtocolObject<dyn UISheetPresentationControllerDetentResolutionContext>,
                >| {
                    // No unwind may cross back into UIKit's layout pass; a
                    // panic (none is expected) makes the detent inactive.
                    catch_unwind(AssertUnwindSafe(|| {
                        // SAFETY: UIKit passes a live, non-null context
                        // object for the duration of this call.
                        let context = unsafe { context.as_ref() };
                        context.maximumDetentValue() * fraction
                    }))
                    // SAFETY: a UIKit constant `CGFloat`, initialized
                    // before any Rust code runs.
                    .unwrap_or(unsafe { UISheetPresentationControllerDetentInactive })
                },
            );
            let object = UISheetPresentationControllerDetent::customDetentWithIdentifier_resolver(
                Some(&identifier(detent)),
                &resolver,
                mtm,
            );
            resolvers.push(resolver);
            object
        }
    }
}

/// The main-queue half of a request: build the sheet, park it as the live
/// entry, then present it (after taking down a displaced one, if any).
fn start(mtm: MainThreadMarker, spec: SheetSpec, tx: Sender<SheetOutcome>, generation: u64) {
    let live = LiveSheet::build(mtm, &spec, tx, generation);
    match install_live(mtm, generation, Box::new(live)) {
        None => present_live(mtm, generation),
        Some(displaced) => {
            let then: AfterDismiss = Box::new(move |mtm| present_live(mtm, generation));
            // Whatever kind `displaced` is, its own `dismiss` takes it down
            // and runs `then` from that dismissal's completion (module
            // doc's *A displaced presentation*).
            displaced.dismiss(mtm, Some(then));
        }
    }
}

/// Present the live entry for `generation` from a freshly discovered anchor.
/// A no-op when the entry is gone (resolved while a displaced sheet was
/// being taken down).
fn present_live(mtm: MainThreadMarker, generation: u64) {
    let Some(live) = take_live_as::<LiveSheet>(mtm, generation) else {
        return;
    };
    let Some(presenter) = presenting_anchor(mtm) else {
        live.finish(Err(PresentError::NoHost));
        return;
    };

    presenter.presentViewController_animated_completion(&live.controller, true, None);
    if live.controller.presentingViewController().is_none() {
        log::warn!(
            "frust-native-widgets: UIKit refused to present the sheet (the presenting controller \
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
        live.controller.observe_disconnect(&scene);
    } else {
        log::debug!(
            "frust-native-widgets: the presenting controller has no window scene; the sheet's \
             scene-disconnect HostLost is not observed"
        );
    }
    park(mtm, generation, live);
}

/// Re-park `live` under `generation`, which nothing else can have claimed
/// during this main-queue turn (anything that was is taken down).
fn park(mtm: MainThreadMarker, generation: u64, live: Box<LiveSheet>) {
    if let Some(unexpected) = install_live(mtm, generation, live) {
        unexpected.dismiss(mtm, None);
    }
}

/// Run a UIKit callback's body without letting a panic unwind into UIKit's
/// frames (`docs/PLUGINS_CODE_STANDARDS.md`'s no-unwind-near-FFI rule). The
/// sender a panicking body held is dropped with it, which resolves its
/// presentation as a platform error rather than leaving it pending.
fn guarded(which: &str, body: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(body)).is_err() {
        log::error!("frust-native-widgets: the sheet's {which} callback panicked");
    }
}

/// Resolve presentation `generation` with `outcome` if it is still the live
/// entry — the path every UIKit callback that ends a sheet already gone from
/// the screen takes.
fn resolve(generation: u64, outcome: Result<SheetOutcome, PresentError>) {
    let Some(mtm) = MainThreadMarker::new() else {
        log::error!("frust-native-widgets: a sheet callback arrived off the main thread");
        return;
    };
    if let Some(live) = take_live_as::<LiveSheet>(mtm, generation) {
        live.finish(outcome);
    }
}

/// Everything one presented sheet keeps alive until its outcome.
struct LiveSheet {
    generation: u64,
    tx: Sender<SheetOutcome>,
    controller: Retained<FrustNativeSheetController>,
    /// Retained by their custom detents too; held here so the entry owns
    /// everything the presentation built.
    _resolvers: Vec<DetentResolver>,
    /// Whether this OS builds custom detents — [`Self::select`] maps a
    /// requested detent the way [`Self::build`] mapped the spec's.
    custom_available: bool,
}

impl LiveSheet {
    fn build(
        mtm: MainThreadMarker,
        spec: &SheetSpec,
        tx: Sender<SheetOutcome>,
        generation: u64,
    ) -> Self {
        let custom_available = custom_detents_available();
        let mut detents: Vec<Detent> = Vec::with_capacity(spec.detents.len());
        for detent in &spec.detents {
            let effective = effective_detent(*detent, custom_available);
            if !detents.contains(&effective) {
                detents.push(effective);
            }
        }

        let controller = FrustNativeSheetController::new(
            mtm,
            SheetIvars {
                generation,
                dismissible: spec.dismissible,
                action_ids: spec.content.actions.iter().map(|a| a.id.clone()).collect(),
                detents: detents
                    .iter()
                    .map(|detent| (*detent, identifier(*detent).to_string()))
                    .collect(),
                on_detent: spec.detent_listener(),
                pending_dismiss: Cell::new(false),
            },
        );
        controller.setView(Some(&content_view(mtm, spec, &controller)));
        controller.setModalPresentationStyle(UIModalPresentationStyle::PageSheet);
        if let Some(dark) = spec.dark {
            controller.setOverrideUserInterfaceStyle(if dark {
                UIUserInterfaceStyle::Dark
            } else {
                UIUserInterfaceStyle::Light
            });
        }

        let mut resolvers = Vec::new();
        if let Some(sheet) = controller.sheetPresentationController() {
            configure_sheet(
                mtm,
                &sheet,
                &controller,
                spec,
                &detents,
                custom_available,
                &mut resolvers,
            );
        } else {
            log::warn!(
                "frust-native-widgets: the sheet controller has no sheetPresentationController; \
                 presenting a plain page sheet without detents"
            );
        }

        Self {
            generation,
            tx,
            controller,
            _resolvers: resolvers,
            custom_available,
        }
    }

    /// Animate to `detent`, if it is one of this sheet's (after the same
    /// availability mapping the spec went through).
    fn select(&self, detent: Detent) {
        let detent = effective_detent(detent, self.custom_available);
        let Some((_, name)) = self
            .controller
            .ivars()
            .detents
            .iter()
            .find(|(known, _)| *known == detent)
        else {
            log::warn!(
                "frust-native-widgets: select_detent({detent:?}) ignored: not one of this sheet's \
                 detents"
            );
            return;
        };
        let Some(sheet) = self.controller.sheetPresentationController() else {
            return;
        };
        let id = NSString::from_str(name);
        let target = sheet.clone();
        let change = RcBlock::new(move || {
            guarded("detent animation", || {
                target.setSelectedDetentIdentifier(Some(&id));
            });
        });
        sheet.animateChanges(&change);
    }

    /// The terminal step: stop observing, deliver `outcome`, and let the
    /// controller go at the end of the current run-loop turn (module doc's
    /// *One object per presentation*).
    fn finish(self, outcome: Result<SheetOutcome, PresentError>) {
        let Self {
            tx,
            controller,
            _resolvers,
            ..
        } = self;
        controller.stop_observing();
        tx.send(outcome);
        let _ = Retained::autorelease_ptr(controller);
    }

    /// Dismiss the sheet, resolving `outcome` once UIKit's dismissal
    /// completes, then run `then` (a displaced sheet's successor). Resolves
    /// at once when there is nothing to wait for: not on screen, or already
    /// on its way out (a second dismissal would be ignored and its
    /// completion never run).
    fn close(
        self: Box<Self>,
        mtm: MainThreadMarker,
        outcome: Result<SheetOutcome, PresentError>,
        then: Option<AfterDismiss>,
    ) {
        let presenter = self.controller.presentingViewController();
        let Some(presenter) = presenter.filter(|_| !self.controller.isBeingDismissed()) else {
            self.finish(outcome);
            if let Some(then) = then {
                then(mtm);
            }
            return;
        };

        let pending = Cell::new(Some((self, outcome, then)));
        let completion = RcBlock::new(move || {
            guarded("dismissal completion", || {
                let Some((live, outcome, then)) = pending.take() else {
                    return;
                };
                live.finish(outcome);
                if let (Some(then), Some(mtm)) = (then, MainThreadMarker::new()) {
                    then(mtm);
                }
            });
        });
        presenter.dismissViewControllerAnimated_completion(true, Some(&completion));
    }

    /// Take the sheet down programmatically → `Dismissed(Programmatic)`, or
    /// re-park it if the platform is already taking it down for some other
    /// reason and nothing is waiting on this one ([`displaced_action`]) —
    /// module doc's *Resolution*'s `SheetHandle::dismiss` entry.
    fn take_down(self: Box<Self>, mtm: MainThreadMarker, then: Option<AfterDismiss>) {
        if displaced_action(self.controller.isBeingDismissed(), then.is_some())
            == DisplacedAction::Repark
        {
            // UIKit is already dismissing it — for the user's swipe, still
            // in progress — whose `presentationControllerDidDismiss:`
            // resolves it next if the gesture completes; a programmatic
            // dismiss loses that race. Record it as pending so a cancelled
            // swipe (UIKit calls `viewDidAppear:` again) retries it instead
            // of silently dropping the request.
            self.controller.ivars().pending_dismiss.set(true);
            let generation = self.generation;
            park(mtm, generation, self);
            return;
        }
        self.close(
            mtm,
            Ok(SheetOutcome::Dismissed(DismissReason::Programmatic)),
            then,
        );
    }
}

impl LivePresentation for LiveSheet {
    fn dismiss(self: Box<Self>, mtm: MainThreadMarker, then: Option<AfterDismiss>) {
        self.take_down(mtm, then);
    }
}

/// Apply the spec's sheet chrome to `sheet` (module doc's *Mapping*).
fn configure_sheet(
    mtm: MainThreadMarker,
    sheet: &UISheetPresentationController,
    controller: &FrustNativeSheetController,
    spec: &SheetSpec,
    detents: &[Detent],
    custom_available: bool,
    resolvers: &mut Vec<DetentResolver>,
) {
    // SAFETY: `delegate` is a weak property; the live entry holds the
    // controller strongly until the presentation resolves, `finish`
    // autoreleases it, and UIKit holds a presented controller itself while
    // it is on screen — so UIKit never messages a freed delegate.
    unsafe {
        sheet.setDelegate(Some(ProtocolObject::from_ref(controller)));
    }
    let objects: Vec<Retained<UISheetPresentationControllerDetent>> = detents
        .iter()
        .map(|detent| detent_object(mtm, *detent, resolvers))
        .collect();
    sheet.setDetents(&NSArray::from_retained_slice(&objects));
    if let Some(selected) = spec.selected {
        let selected = effective_detent(selected, custom_available);
        sheet.setSelectedDetentIdentifier(Some(&identifier(selected)));
    }
    if let Some(undimmed) = spec.largest_undimmed {
        let undimmed = effective_detent(undimmed, custom_available);
        sheet.setLargestUndimmedDetentIdentifier(Some(&identifier(undimmed)));
    }
    sheet.setPrefersGrabberVisible(spec.grabber);
    sheet.setPrefersScrollingExpandsWhenScrolledToEdge(spec.scrolling_expands);
    sheet.setPrefersEdgeAttachedInCompactHeight(true);
    sheet.setWidthFollowsPreferredContentSizeWhenEdgeAttached(true);
    if let Some(radius) = spec.corner_radius {
        sheet.setPreferredCornerRadius(radius);
    }
}

/// The sheet's root view: a system-background `UIView` holding the
/// top-aligned `UIStackView` of rows (module doc's *Mapping*).
fn content_view(
    mtm: MainThreadMarker,
    spec: &SheetSpec,
    target: &FrustNativeSheetController,
) -> Retained<UIView> {
    let root = UIView::new(mtm);
    root.setBackgroundColor(Some(&UIColor::systemBackgroundColor()));
    if let Some(tint) = spec.tint {
        // SAFETY: `tintColor` is documented nullable (nil restores the
        // inherited tint); a non-nil colour is always accepted.
        unsafe { root.setTintColor(Some(&ui_color(tint as i32))) };
    }

    let stack = UIStackView::new(mtm);
    stack.setAxis(UILayoutConstraintAxis::Vertical);
    stack.setAlignment(UIStackViewAlignment::Fill);
    stack.setSpacing(ROW_SPACING);
    stack.setTranslatesAutoresizingMaskIntoConstraints(false);
    root.addSubview(&stack);

    let mut constraints: Vec<Retained<NSLayoutConstraint>> = Vec::new();
    let content = &spec.content;
    if let Some(title) = content.title.as_deref().filter(|t| !t.is_empty()) {
        let label = wrapping_label(mtm, title);
        set_label_font(&label, &UIFont::boldSystemFontOfSize(22.0));
        stack.addArrangedSubview(&label);
    }
    if let Some(message) = content.message.as_deref().filter(|m| !m.is_empty()) {
        let label = wrapping_label(mtm, message);
        set_label_font(&label, &UIFont::systemFontOfSize(17.0));
        set_label_text_color(&label, &UIColor::secondaryLabelColor());
        stack.addArrangedSubview(&label);
    }
    if let Some(bytes) = content.image.as_deref() {
        match UIImage::imageWithData(&NSData::with_bytes(bytes)) {
            Some(image) => {
                let view = UIImageView::initWithImage(UIImageView::alloc(mtm), Some(&image));
                view.setContentMode(UIViewContentMode::ScaleAspectFit);
                view.setClipsToBounds(true);
                constraints.push(
                    view.heightAnchor()
                        .constraintLessThanOrEqualToConstant(IMAGE_MAX_HEIGHT),
                );
                stack.addArrangedSubview(&view);
            }
            None => log::warn!(
                "frust-native-widgets: the sheet's image bytes did not decode; the image row is \
                 omitted"
            ),
        }
    }
    for (index, action) in content.actions.iter().enumerate() {
        let button = UIButton::buttonWithType(UIButtonType::System, mtm);
        button.setTitle_forState(
            Some(&NSString::from_str(&action.label)),
            UIControlState::Normal,
        );
        // Spec indices are bounded by `MAX_SHEET_ACTIONS`, so this fits.
        button.setTag(index as isize);
        match action.role {
            ActionRole::Default => {}
            ActionRole::Destructive => button
                .setTitleColor_forState(Some(&UIColor::systemRedColor()), UIControlState::Normal),
            ActionRole::Cancel => button.setTitleColor_forState(
                Some(&UIColor::secondaryLabelColor()),
                UIControlState::Normal,
            ),
        }
        if let Some(label) = button.titleLabel() {
            set_label_font(&label, &UIFont::boldSystemFontOfSize(17.0));
        }
        let receiver: &AnyObject = target;
        // SAFETY: `receiver` is the live sheet controller implementing
        // `frustSheetAction:` (defined below, taking the sender). UIKit
        // holds a control's target weakly; the controller owns the button
        // through its view, so the target outlives every action it can
        // receive.
        unsafe {
            button.addTarget_action_forControlEvents(
                Some(receiver),
                sel!(frustSheetAction:),
                UIControlEvents::TouchUpInside,
            );
        }
        constraints.push(
            button
                .heightAnchor()
                .constraintGreaterThanOrEqualToConstant(ACTION_MIN_HEIGHT),
        );
        stack.addArrangedSubview(&button);
    }

    let guide = root.safeAreaLayoutGuide();
    constraints.extend([
        stack
            .topAnchor()
            .constraintEqualToAnchor_constant(&guide.topAnchor(), TOP_INSET),
        stack
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&guide.leadingAnchor(), SIDE_INSET),
        guide
            .trailingAnchor()
            .constraintEqualToAnchor_constant(&stack.trailingAnchor(), SIDE_INSET),
        guide
            .bottomAnchor()
            .constraintGreaterThanOrEqualToAnchor_constant(&stack.bottomAnchor(), BOTTOM_INSET),
    ]);
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&constraints), mtm);
    root
}

fn wrapping_label(mtm: MainThreadMarker, text: &str) -> Retained<UILabel> {
    let label = UILabel::new(mtm);
    label.setText(Some(&NSString::from_str(text)));
    label.setNumberOfLines(0);
    label
}

/// The controller's per-presentation state (module doc's *One object per
/// presentation*).
struct SheetIvars {
    generation: u64,
    dismissible: bool,
    /// The spec's action ids, indexed by each button's tag.
    action_ids: Vec<String>,
    /// Every detent the sheet was given (after availability mapping) with
    /// the identifier UIKit reports it by.
    detents: Vec<(Detent, String)>,
    on_detent: Option<Arc<dyn Fn(Detent) + Send + Sync>>,
    /// Set when a programmatic dismiss ([`LiveSheet::take_down`]) loses its
    /// race against an interactive swipe already in progress; retried once
    /// from `viewDidAppear:`, which UIKit calls again if the swipe is
    /// cancelled (module doc's *Resolution*'s `SheetHandle::dismiss` entry).
    pending_dismiss: Cell<bool>,
}

define_class!(
    // SAFETY:
    // - `UIViewController` may be subclassed (it is designed for it); this
    //   subclass overrides `viewDidDisappear:` and `viewDidAppear:`, each
    //   calling super first.
    // - The ivars are plain Rust values; the macro's generated `dealloc`
    //   drops them, and none of them touches the object being deallocated.
    #[unsafe(super(UIViewController, UIResponder, NSObject))]
    // UIKit controllers are main-thread-only, and the arm builds it there
    // (`Self::alloc(mtm)`).
    #[thread_kind = MainThreadOnly]
    #[ivars = SheetIvars]
    struct FrustNativeSheetController;

    unsafe impl NSObjectProtocol for FrustNativeSheetController {}

    unsafe impl UIAdaptivePresentationControllerDelegate for FrustNativeSheetController {
        /// A swipe-down (or a tap outside a form sheet): allowed only for a
        /// dismissible spec.
        #[unsafe(method(presentationControllerShouldDismiss:))]
        fn presentation_controller_should_dismiss(
            &self,
            _controller: &UIPresentationController,
        ) -> bool {
            self.ivars().dismissible
        }

        /// The sheet went away through the user → `Dismissed(User)`.
        #[unsafe(method(presentationControllerDidDismiss:))]
        fn presentation_controller_did_dismiss(&self, _controller: &UIPresentationController) {
            let generation = self.ivars().generation;
            guarded("swipe-down", || {
                resolve(generation, Ok(SheetOutcome::Dismissed(DismissReason::User)));
            });
        }
    }

    unsafe impl UISheetPresentationControllerDelegate for FrustNativeSheetController {
        /// The user moved the sheet to another detent → the listener; the
        /// sheet stays live.
        #[unsafe(method(sheetPresentationControllerDidChangeSelectedDetentIdentifier:))]
        fn sheet_did_change_detent(&self, sheet: &UISheetPresentationController) {
            guarded("detent change", || {
                let Some(selected) = sheet.selectedDetentIdentifier() else {
                    return;
                };
                let selected = selected.to_string();
                let ivars = self.ivars();
                let detent = ivars
                    .detents
                    .iter()
                    .find(|(_, name)| *name == selected)
                    .map(|(detent, _)| *detent);
                if let (Some(detent), Some(listener)) = (detent, ivars.on_detent.as_ref()) {
                    listener(detent);
                }
            });
        }
    }

    impl FrustNativeSheetController {
        /// An action row's `TouchUpInside` → dismiss, then `Action(id)`.
        #[unsafe(method(frustSheetAction:))]
        fn sheet_action(&self, sender: &UIButton) {
            let generation = self.ivars().generation;
            let id = usize::try_from(sender.tag())
                .ok()
                .and_then(|index| self.ivars().action_ids.get(index))
                .cloned();
            guarded("action", || {
                let Some(id) = id else {
                    log::error!("frust-native-widgets: a sheet action fired with an unknown tag");
                    return;
                };
                let Some(mtm) = MainThreadMarker::new() else {
                    return;
                };
                if let Some(live) = take_live_as::<LiveSheet>(mtm, generation) {
                    live.close(mtm, Ok(SheetOutcome::Action(id)), None);
                }
            });
        }

        /// The sheet left the screen. Checked one main-queue turn later: a
        /// swipe-down's own callback (same transition) resolves first; what
        /// is still live then with no presenter lost its host.
        #[unsafe(method(viewDidDisappear:))]
        fn view_did_disappear(&self, animated: bool) {
            // SAFETY: forwarding the same selector with its one `BOOL`
            // argument to `UIViewController`'s implementation, which UIKit
            // requires an override to call.
            let _: () = unsafe { msg_send![super(self), viewDidDisappear: animated] };
            let generation = self.ivars().generation;
            guarded("disappearance", || {
                on_main(move |mtm| {
                    let Some(live) = take_live_as::<LiveSheet>(mtm, generation) else {
                        return;
                    };
                    if live.controller.presentingViewController().is_some() {
                        // Covered by another presentation, not gone.
                        park(mtm, generation, live);
                    } else {
                        live.finish(Ok(SheetOutcome::HostLost));
                    }
                });
            });
        }

        /// `UISceneDidDisconnectNotification` for the presenting scene →
        /// `HostLost`.
        #[unsafe(method(sceneDidDisconnect:))]
        fn scene_did_disconnect(&self, _notification: &NSNotification) {
            let generation = self.ivars().generation;
            guarded("scene disconnect", || {
                resolve(generation, Ok(SheetOutcome::HostLost));
            });
        }

        /// The sheet is back on screen — including a cancelled interactive
        /// swipe bringing it back. Retries a programmatic dismiss that lost
        /// its race against that swipe ([`LiveSheet::take_down`]); a no-op
        /// otherwise, including the sheet's own first appearance.
        #[unsafe(method(viewDidAppear:))]
        fn view_did_appear(&self, animated: bool) {
            // SAFETY: forwarding the same selector with its one `BOOL`
            // argument to `UIViewController`'s implementation, which UIKit
            // requires an override to call.
            let _: () = unsafe { msg_send![super(self), viewDidAppear: animated] };
            if !self.ivars().pending_dismiss.replace(false) {
                return;
            }
            let generation = self.ivars().generation;
            guarded("pending dismiss retry", || {
                let Some(mtm) = MainThreadMarker::new() else {
                    return;
                };
                if let Some(live) = take_live_as::<LiveSheet>(mtm, generation) {
                    live.take_down(mtm, None);
                }
            });
        }
    }
);

impl FrustNativeSheetController {
    fn new(mtm: MainThreadMarker, ivars: SheetIvars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: `UIViewController`'s `init` (which forwards to its
        // designated `initWithNibName:nil bundle:nil` — no nib; the view is
        // assigned right after), called on a freshly allocated instance
        // whose ivars are already set (the alert delegate's idiom).
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
        // observer before the live entry lets the controller go.
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
