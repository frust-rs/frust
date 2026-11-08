//! iOS: the system photo picker (`PHPickerViewController`).
//!
//! # No permission
//!
//! `PHPickerConfiguration`'s plain `init` uses the system photo library, and
//! the picker runs out of process, so no photo-library authorization and no
//! Info.plist usage string are needed (`PHPicker.h`). The same header says this
//! configuration never returns asset identifiers, so every
//! [`PickedItem::asset_id`] is `None`.
//!
//! # Threading
//!
//! [`pick`] parks its caller until the user answers, and the picker needs the
//! main thread free to appear, so it refuses the main thread with
//! [`PhotoError::UiThread`]. It enqueues the build-and-present sequence on the
//! main queue and waits on a channel:
//!
//! 1. The main-queue block sends [`Event::Failed`] when it cannot present (no
//!    presenter, PhotosUI not loaded); otherwise it hands the sender to the
//!    picker's delegate.
//! 2. The delegate's `picker:didFinishPicking:` (main thread) dismisses the
//!    picker, sends [`Event::Finished`] with the result count and starts one
//!    file load per result.
//! 3. Each load's completion handler runs on an item-provider queue, copies the
//!    file into this call's pick-cache directory before returning, and sends
//!    [`Event::Item`].
//!
//! The delegate is owned by the picker itself, so a picker that goes away
//! without answering drops the last sender and the waiting call returns an
//! empty `Vec`, the same as a cancel. One pick runs at a time; a second
//! concurrent call, or one made while `clear_pick_cache` runs, fails with
//! [`PhotoError::Busy`].
//!
//! # Results
//!
//! Items come back in selection order. A single item that cannot be copied is
//! logged and skipped; only when every item fails does the call return
//! [`PhotoError::Platform`]. A cancelled or failed call removes its empty
//! cache directory.

#[path = "apple_picker.rs"]
mod apple_picker;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2_ui_kit::{
    UIApplication, UISceneActivationState, UIViewController, UIWindow, UIWindowScene,
};

use crate::pick_cache;
use crate::{PhotoError, PickOptions, PickedItem};

/// One message from the main-queue presentation or an item handler to the
/// waiting [`pick`] call.
pub(super) enum Event {
    /// The picker could not be shown.
    Failed(PhotoError),
    /// The user finished; this many items follow (`0` = cancelled).
    Finished(usize),
    /// The outcome for the item at this selection index.
    Item(usize, Result<PickedItem, String>),
}

pub(crate) fn pick(options: &PickOptions) -> Result<Vec<PickedItem>, PhotoError> {
    if MainThreadMarker::new().is_some() {
        return Err(PhotoError::UiThread);
    }
    let _in_flight = pick_cache::PickGuard::acquire()?;
    let dir = pick_cache::new_call_dir()?;
    let (tx, rx) = mpsc::channel();
    let options = *options;
    let call_dir = dir.clone();
    DispatchQueue::main().exec_async(move || {
        // SAFETY: this closure is submitted to `dispatch_get_main_queue()`,
        // which always executes its blocks on the process's main thread, so
        // constructing a `MainThreadMarker` here without re-checking is sound.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        // A panic must not unwind out of a dispatch block (no unwind across FFI).
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            present_on_main(mtm, options, call_dir, &tx)
        }))
        .unwrap_or_else(|_| {
            Err(PhotoError::Platform(
                "panic while presenting the photo picker".to_owned(),
            ))
        });
        if let Err(e) = outcome {
            let _ = tx.send(Event::Failed(e));
        }
    });
    let result = collect(&rx);
    if !matches!(&result, Ok(items) if !items.is_empty()) {
        // Nothing was kept: drop the empty per-call directory. Best effort.
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

/// Build the picker and present it. Runs on the main queue.
fn present_on_main(
    mtm: MainThreadMarker,
    options: PickOptions,
    dir: PathBuf,
    tx: &Sender<Event>,
) -> Result<(), PhotoError> {
    // The bindings look their classes up by name and panic when the class is
    // missing, so check first: PhotosUI must be linked into the app.
    if AnyClass::get(c"PHPickerViewController").is_none()
        || AnyClass::get(c"PHPickerConfiguration").is_none()
    {
        return Err(PhotoError::Platform(
            "PhotosUI is not loaded; link PhotosUI.framework".to_owned(),
        ));
    }
    let presenter = presenter(mtm).ok_or(PhotoError::NoPresenter)?;
    let picker = apple_picker::Picker::new(mtm, &options, tx.clone(), dir);
    presenter.presentViewController_animated_completion(&picker.controller, true, None);
    // UIKit refuses a presentation (logging a warning, never calling back)
    // when the presenter is not in a window or is mid-transition. It sets
    // `presentingViewController` when it accepts one, so a missing value means
    // no `didFinishPicking:` will ever arrive.
    if picker.controller.presentingViewController().is_none() {
        picker.abandon();
        return Err(PhotoError::NoPresenter);
    }
    Ok(())
}

/// Wait for the outcome of one pick (module doc's *Results*).
fn collect(rx: &Receiver<Event>) -> Result<Vec<PickedItem>, PhotoError> {
    let count = loop {
        match rx.recv() {
            Ok(Event::Failed(e)) => return Err(e),
            Ok(Event::Finished(count)) => break count,
            Ok(Event::Item(..)) => continue,
            // The picker went away without answering: a cancel.
            Err(_) => return Ok(Vec::new()),
        }
    };
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut slots: Vec<Option<PickedItem>> = (0..count).map(|_| None).collect();
    let mut answered = 0;
    while answered < count {
        match rx.recv() {
            Ok(Event::Item(index, outcome)) => {
                answered += 1;
                match outcome {
                    Ok(item) => {
                        if let Some(slot) = slots.get_mut(index) {
                            *slot = Some(item);
                        }
                    }
                    Err(reason) => {
                        log::warn!("photo picker: skipping item {index}: {reason}");
                    }
                }
            }
            Ok(_) => continue,
            // Every handler is gone; whatever did not answer is lost.
            Err(_) => {
                log::warn!(
                    "photo picker: {} of {count} items never answered",
                    count - answered
                );
                break;
            }
        }
    }
    let items: Vec<PickedItem> = slots.into_iter().flatten().collect();
    if items.is_empty() {
        return Err(PhotoError::Platform(format!(
            "none of the {count} picked items could be copied"
        )));
    }
    Ok(items)
}

/// The top view controller of the anchor window: its `rootViewController`,
/// followed up the `presentedViewController` chain past anything being
/// dismissed. `None` when the app has no window with a root controller yet.
fn presenter(mtm: MainThreadMarker) -> Option<Retained<UIViewController>> {
    let mut top = anchor_window(mtm)?.rootViewController()?;
    while let Some(next) = top.presentedViewController() {
        if next.isBeingDismissed() {
            break;
        }
        top = next;
    }
    Some(top)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MediaKind;

    fn item(name: &str) -> PickedItem {
        PickedItem {
            path: PathBuf::from(name),
            kind: MediaKind::Image,
            file_name: Some(name.to_owned()),
            mime: None,
            asset_id: None,
            is_copy: true,
        }
    }

    fn names(items: &[PickedItem]) -> Vec<&str> {
        items
            .iter()
            .filter_map(|i| i.file_name.as_deref())
            .collect()
    }

    #[test]
    fn items_come_back_in_selection_order() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Finished(3)).unwrap();
        tx.send(Event::Item(2, Ok(item("c")))).unwrap();
        tx.send(Event::Item(0, Ok(item("a")))).unwrap();
        tx.send(Event::Item(1, Ok(item("b")))).unwrap();
        assert_eq!(names(&collect(&rx).unwrap()), ["a", "b", "c"]);
    }

    #[test]
    fn a_failed_item_is_skipped() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Finished(2)).unwrap();
        tx.send(Event::Item(0, Err("load failed".to_owned())))
            .unwrap();
        tx.send(Event::Item(1, Ok(item("b")))).unwrap();
        assert_eq!(names(&collect(&rx).unwrap()), ["b"]);
    }

    #[test]
    fn all_failed_is_a_platform_error() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Finished(1)).unwrap();
        tx.send(Event::Item(0, Err("load failed".to_owned())))
            .unwrap();
        assert!(matches!(collect(&rx), Err(PhotoError::Platform(_))));
    }

    #[test]
    fn cancel_and_vanished_picker_are_empty() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Finished(0)).unwrap();
        assert!(collect(&rx).unwrap().is_empty());
        let (tx, rx) = mpsc::channel::<Event>();
        drop(tx);
        assert!(collect(&rx).unwrap().is_empty());
    }

    #[test]
    fn a_presentation_failure_is_returned() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Failed(PhotoError::NoPresenter)).unwrap();
        assert_eq!(collect(&rx).unwrap_err(), PhotoError::NoPresenter);
    }

    #[test]
    fn lost_handlers_keep_what_arrived() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Finished(2)).unwrap();
        tx.send(Event::Item(1, Ok(item("b")))).unwrap();
        drop(tx);
        assert_eq!(names(&collect(&rx).unwrap()), ["b"]);
    }

    #[test]
    fn only_one_pick_at_a_time() {
        let first = pick_cache::PickGuard::acquire().unwrap();
        assert_eq!(
            pick_cache::PickGuard::acquire().err(),
            Some(PhotoError::Busy)
        );
        drop(first);
        assert!(pick_cache::PickGuard::acquire().is_ok());
    }
}
