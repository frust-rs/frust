//! The iOS backend — `UIActivityViewController` presented from the key
//! window's top view controller, via `objc2-ui-kit`.
//!
//! `#[cfg(target_os = "ios")]` only: UIKit does not exist on macOS, which
//! takes the desktop clipboard fallback instead.
//!
//! # Main-thread dispatch
//!
//! Every UIKit type touched here is `MainThreadOnly`. [`share`] therefore
//! bounces the whole build-and-present sequence onto the main queue with
//! `DispatchQueue::exec_async` and returns `Ok(Presented)` once that block is
//! enqueued — never blocking the caller, whichever thread it is on (the same
//! shape as `frust-url-launcher`'s Apple backend).
//!
//! # Presenter
//!
//! The window is the key window of a foreground-active `UIWindowScene`
//! (falling back to its first window, then to any scene's window — the
//! `frust-auth-session` walk); the presenter is that window's
//! `rootViewController`, followed up its `presentedViewController` chain past
//! anything not being dismissed (the `frust-native-widgets` walk). [`share`]
//! runs the walk synchronously when it is already on the main thread, so a
//! missing presenter surfaces as [`ShareError::NoPresenter`]; from any other
//! thread the walk runs inside the enqueued block and a miss is dropped.
//!
//! # iPad popover
//!
//! On an iPad (`UIUserInterfaceIdiom::Pad`) UIKit raises unless the activity
//! sheet's popover has a source, so `sourceView` is always the presenter's
//! view and `sourceRect` is [`crate::popover_source`]'s rect: the app's
//! [`ShareAnchor`] (logical points are UIKit points, 1:1, in the root view's
//! space) or, without one, the root view's centre with no arrow.

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_foundation::{
    NSArray, NSObjectNSKeyValueCoding, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use objc2_ui_kit::{
    UIActivityViewController, UIApplication, UIDevice, UIPopoverArrowDirection,
    UISceneActivationState, UIUserInterfaceIdiom, UIView, UIViewController, UIWindow,
    UIWindowScene,
};

use crate::{PayloadKind, ShareAnchor, ShareError, ShareOutcome, popover_source};

/// Enqueue the activity sheet on the main queue (module doc's *Main-thread
/// dispatch*).
///
/// # Errors
/// [`ShareError::NoPresenter`] when called on the main thread and no window
/// has a root view controller yet.
pub(crate) fn share(
    text: &str,
    kind: PayloadKind,
    subject: Option<&str>,
    anchor: Option<&ShareAnchor>,
) -> Result<ShareOutcome, ShareError> {
    if let Some(mtm) = MainThreadMarker::new()
        && presenter(mtm).is_none()
    {
        return Err(ShareError::NoPresenter);
    }
    let text = text.to_owned();
    let subject = subject.map(str::to_owned);
    let anchor = anchor.copied();
    DispatchQueue::main().exec_async(move || {
        // SAFETY: this closure is submitted to `dispatch_get_main_queue()`,
        // which always executes its blocks on the process's main thread, so
        // constructing a `MainThreadMarker` here without re-checking is sound.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        present_on_main(mtm, &text, kind, subject.as_deref(), anchor.as_ref());
    });
    Ok(ShareOutcome::Presented)
}

/// Build the controller and present it. Runs on the main queue; every miss
/// here is dropped, as no caller is left to report to.
fn present_on_main(
    mtm: MainThreadMarker,
    text: &str,
    kind: PayloadKind,
    subject: Option<&str>,
    anchor: Option<&ShareAnchor>,
) {
    let Some(presenter) = presenter(mtm) else {
        return;
    };
    let ns_text = NSString::from_str(text);
    let item: Retained<AnyObject> = match kind {
        PayloadKind::Text => ns_text.into(),
        PayloadKind::Url => match NSURL::URLWithString(&ns_text) {
            Some(url) => url.into(),
            None => ns_text.into(),
        },
    };
    let items = NSArray::<AnyObject>::from_retained_slice(&[item]);
    // SAFETY: `activity_items` is an `NSArray` of `NSString`/`NSURL`, the item
    // types the binding's "correct type" obligation and UIKit's
    // documentation accept; `application_activities` is `None`.
    let controller = unsafe {
        UIActivityViewController::initWithActivityItems_applicationActivities(
            UIActivityViewController::alloc(mtm),
            &items,
            None,
        )
    };

    if let Some(subject) = subject.filter(|s| !s.is_empty())
        && controller.respondsToSelector(sel!(setSubject:))
    {
        let subject = NSString::from_str(subject);
        // SAFETY: guarded by `respondsToSelector(setSubject:)`, so KVC finds a
        // setter rather than raising `NSUndefinedKeyException`; the value is an
        // `NSString`, the `subject` property's type.
        unsafe {
            controller.setValue_forKey(Some(&subject), &NSString::from_str("subject"));
        }
    }

    if UIDevice::currentDevice(mtm).userInterfaceIdiom() == UIUserInterfaceIdiom::Pad {
        let (Some(popover), Some(view)) =
            (controller.popoverPresentationController(), presenter.view)
        else {
            // UIKit raises on a sourceless iPad popover; dropping is safer.
            return;
        };
        let source = popover_source(anchor, kurbo_rect(presenter.root_view.bounds()));
        let rect = view.convertRect_fromView(ns_rect(source.rect), Some(&presenter.root_view));
        popover.setSourceView(Some(&view));
        popover.setSourceRect(rect);
        if !source.arrows {
            popover.setPermittedArrowDirections(UIPopoverArrowDirection(0));
        }
    }

    presenter
        .controller
        .presentViewController_animated_completion(&controller, true, None);
}

/// The controller to present from, its view, and the root view whose space
/// [`ShareAnchor`] is in.
struct Presenter {
    controller: Retained<UIViewController>,
    view: Option<Retained<UIView>>,
    root_view: Retained<UIView>,
}

/// The module doc's *Presenter* walk; `None` when the app has no window with
/// a root view controller (and view) yet.
fn presenter(mtm: MainThreadMarker) -> Option<Presenter> {
    let root = anchor_window(mtm)?.rootViewController()?;
    let root_view = root.view()?;
    let mut top = root;
    while let Some(next) = top.presentedViewController() {
        if next.isBeingDismissed() {
            break;
        }
        top = next;
    }
    Some(Presenter {
        view: top.view(),
        controller: top,
        root_view,
    })
}

/// The key window of a foreground-active scene, falling back to any window
/// of one, then to any window scene's window.
fn anchor_window(mtm: MainThreadMarker) -> Option<Retained<UIWindow>> {
    let scenes = UIApplication::sharedApplication(mtm).connectedScenes();
    let mut fallback = None;
    for scene in scenes.iter() {
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

fn kurbo_rect(r: NSRect) -> kurbo::Rect {
    kurbo::Rect::from_origin_size((r.origin.x, r.origin.y), (r.size.width, r.size.height))
}

fn ns_rect(r: kurbo::Rect) -> NSRect {
    NSRect::new(NSPoint::new(r.x0, r.y0), NSSize::new(r.width(), r.height()))
}
