//! iOS: the system photo picker (`PHPickerViewController`).
//!
//! # No permission
//!
//! `PHPickerConfiguration`'s plain `init` uses the system photo library, and
//! the picker runs out of process, so no photo-library authorization and no
//! Info.plist usage string are needed (`PHPicker.h`). The same header says this
//! configuration never returns asset identifiers, so [`PickedItem::asset_id`]
//! is `None`. With the `library` feature the configuration is built on the
//! shared photo library instead (`initWithPhotoLibrary:`) and an item carries
//! the identifier the picker reports. Picking still needs no authorization;
//! resolving that identifier through browsing does.
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
//!    picker's delegate. The picker is a hosted scene that UIKit attaches only
//!    once its out-of-process service connects, so the presentation is
//!    confirmed by its completion block, not right after the call: if
//!    [`PRESENT_GRACE`] passes without it, UIKit refused the presentation and
//!    the call fails with [`PhotoError::NoPresenter`] (a picker attached after
//!    that is dismissed at once). That watch holds a sender and the picker
//!    until it fires, so a picker that goes away sooner answers by then.
//! 2. The delegate's `picker:didFinishPicking:` (main thread) dismisses the
//!    picker, sends [`Event::Finished`] with the result count and starts one
//!    file load per result, keeping each load's `NSProgress` in [`Shared`]. A
//!    swipe-down never calls it: the delegate is also the sheet's presentation
//!    controller delegate, and `presentationControllerDidDismiss:` sends
//!    `Finished(0)`. The first answer takes the one sender.
//! 3. Each load's completion handler runs on an item-provider queue, copies the
//!    file into this call's pick-cache directory before returning, and sends
//!    [`Event::Item`].
//! 4. Meanwhile the waiting thread reports [`PickProgress`] on its own thread:
//!    `done: 0, fraction: 0.0` at once, then every [`POLL`] and on each item
//!    the answered count and [`overall_fraction`], ending at `1.0`.
//!    `NSProgress` is thread-safe (Foundation), so it is read off the main
//!    queue.
//! 5. A cancel runs the token's hook on the cancelling thread: it marks
//!    [`Shared`] cancelled, enqueues on the main queue the abandon-then-dismiss
//!    of a still-presented picker and `cancel` on each kept `NSProgress`. Both
//!    waits check the mark on every wake-up (at most [`POLL`] apart), and
//!    [`pick`] checks the token once more after the wait, so a cancel that races
//!    the last answer still returns [`PhotoError::Cancelled`]. A handler that
//!    runs later deletes its copy.
//!
//! A picker released without answering sends `Finished(0)` from its delegate,
//! the same as a dismissal. One pick runs at a time, until [`pick`] returns; a
//! second concurrent call, or one made while `clear_pick_cache` runs, fails
//! with [`PhotoError::Busy`].
//!
//! # Results
//!
//! Items come back in selection order. A single item that cannot be copied is
//! logged and skipped; only when every item fails does the call return
//! [`PhotoError::Platform`]. A cancelled or failed call removes its cache
//! directory.

#[cfg(feature = "library")]
#[path = "apple_library.rs"]
mod apple_library;
#[path = "apple_picker.rs"]
mod apple_picker;

#[cfg(feature = "library")]
pub(crate) use apple_library::{albums, assets, manage_limited_selection, thumbnail};

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchTime, MainThreadBound};
use objc2::MainThreadMarker;
use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyClass;
use objc2_foundation::NSProgress;
use objc2_ui_kit::{
    UIApplication, UISceneActivationState, UIViewController, UIWindow, UIWindowScene,
};

use crate::cancel::ProgressThrottle;
use crate::pick_cache;
use crate::{CancelToken, PhotoError, PickOptions, PickProgress, PickedItem};

/// How long UIKit may take to attach the picker before the call gives up.
const PRESENT_GRACE: Duration = Duration::from_secs(10);

/// How often the loading phase samples the loads' progress.
const POLL: Duration = Duration::from_millis(100);

/// Where one picker's presentation stands (module doc, *Threading* step 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    /// Requested; UIKit has not attached it yet.
    Pending,
    /// The presentation's completion block ran.
    Presented,
    /// [`PRESENT_GRACE`] passed first and the call failed.
    GaveUp,
}

/// One message from the main-queue presentation, an item handler or the
/// cancel hook to the waiting [`pick`] call.
pub(super) enum Event {
    /// The picker could not be shown.
    Failed(PhotoError),
    /// The user finished; this many items follow (`0` = dismissed).
    Finished(usize),
    /// The outcome for the item at this selection index.
    Item(usize, Result<PickedItem, String>),
}

/// What one pick shares between the waiting thread, the main queue, the item
/// handlers and the cancel hook.
#[derive(Default)]
pub(super) struct Shared {
    /// Set by the cancel hook before anything else it does.
    cancelled: AtomicBool,
    /// Each started load's progress, by selection index. The binding is `Send`
    /// and `Sync` because Foundation documents `NSProgress` as thread-safe.
    loads: Mutex<Vec<(usize, Retained<NSProgress>)>>,
    /// The presented picker. Set and taken on the main queue only.
    picker: Mutex<Option<MainThreadBound<apple_picker::WeakPicker>>>,
}

impl Shared {
    pub(super) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub(super) fn keep_load(&self, index: usize, progress: Retained<NSProgress>) {
        lock(&self.loads).push((index, progress));
    }

    /// The `fractionCompleted` of the load for `index`, once it has started.
    fn load_fraction(&self, index: usize) -> Option<f64> {
        lock(&self.loads)
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, progress)| progress.fractionCompleted())
    }

    /// The cancel's main-queue step (module doc, *Threading* step 5).
    fn cancel_on_main(&self, mtm: MainThreadMarker) {
        if let Some(picker) = lock(&self.picker).take() {
            picker.get(mtm).cancel();
        }
        let loads: Vec<Retained<NSProgress>> =
            lock(&self.loads).iter().map(|(_, p)| p.clone()).collect();
        for progress in loads {
            progress.cancel();
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The token hook: runs once, on whichever thread cancels.
fn on_cancel(shared: &Arc<Shared>) {
    shared.cancelled.store(true, Ordering::SeqCst);
    let main = Arc::clone(shared);
    DispatchQueue::main().exec_async(move || {
        // SAFETY: the block is submitted to the main queue, which always runs
        // on the process's main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        if catch_unwind(AssertUnwindSafe(|| main.cancel_on_main(mtm))).is_err() {
            log::error!("photo picker: panic while cancelling the pick");
        }
    });
}

pub(crate) fn pick(options: &PickOptions) -> Result<Vec<PickedItem>, PhotoError> {
    if MainThreadMarker::new().is_some() {
        return Err(PhotoError::UiThread);
    }
    if options
        .cancel
        .as_ref()
        .is_some_and(CancelToken::is_cancelled)
    {
        return Err(PhotoError::Cancelled);
    }
    let _in_flight = pick_cache::PickGuard::acquire()?;
    let dir = pick_cache::new_call_dir()?;
    let (tx, rx) = mpsc::channel();
    let shared = Arc::new(Shared::default());
    if let Some(token) = &options.cancel {
        let shared = Arc::clone(&shared);
        token.set_hook(move || on_cancel(&shared));
    }
    let present = {
        let options = options.clone();
        let call_dir = dir.clone();
        let shared = Arc::clone(&shared);
        move || {
            // SAFETY: this closure is submitted to `dispatch_get_main_queue()`,
            // which always executes its blocks on the process's main thread, so
            // constructing a `MainThreadMarker` here without re-checking is sound.
            let mtm = unsafe { MainThreadMarker::new_unchecked() };
            // A panic must not unwind out of a dispatch block (no unwind across FFI).
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                present_on_main(mtm, options, call_dir, &tx, &shared)
            }))
            .unwrap_or_else(|_| {
                Err(PhotoError::Platform(
                    "panic while presenting the photo picker".to_owned(),
                ))
            });
            if let Err(e) = outcome {
                let _ = tx.send(Event::Failed(e));
            }
        }
    };
    DispatchQueue::main().exec_async(present);
    let throttle = options.progress.clone().map(ProgressThrottle::new);
    let result = collect(
        &rx,
        throttle.as_ref(),
        &|index| shared.load_fraction(index),
        &|| shared.is_cancelled(),
    );
    if let Some(token) = &options.cancel {
        token.clear_hook();
    }
    // Checked after the hook is cleared, so a cancel that raced the last answer still wins.
    let cancelled = options
        .cancel
        .as_ref()
        .is_some_and(CancelToken::is_cancelled);
    let result = if cancelled {
        Err(PhotoError::Cancelled)
    } else {
        result
    };
    // Queued after the presentation and any cancel step, so the picker handle
    // is released on the main thread once nothing needs it.
    let release = Arc::clone(&shared);
    DispatchQueue::main().exec_async(move || drop(lock(&release.picker).take()));
    if !matches!(&result, Ok(items) if !items.is_empty()) {
        // Nothing was kept: drop the per-call directory. Best effort; a load
        // handler still running deletes its own copy (`Shared::cancelled`).
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
    shared: &Arc<Shared>,
) -> Result<(), PhotoError> {
    if shared.is_cancelled() {
        // Cancelled before the picker was built; the call has returned.
        return Ok(());
    }
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
    let picker = apple_picker::Picker::new(mtm, &options, tx.clone(), dir, Arc::clone(shared));
    let shown = Rc::new(Cell::new(Shown::Pending));
    let on_shown = {
        let shown = Rc::clone(&shown);
        let shared = Arc::clone(shared);
        let controller = Weak::from_retained(&picker.controller);
        RcBlock::new(move || {
            let gave_up = shown.get() == Shown::GaveUp;
            if !gave_up {
                shown.set(Shown::Presented);
            }
            if gave_up || shared.is_cancelled() {
                // Attached after the call already ended: take it down again.
                if let Some(controller) = controller.load() {
                    controller.dismissViewControllerAnimated_completion(false, None);
                }
            }
        })
    };
    presenter.presentViewController_animated_completion(&picker.controller, true, Some(&on_shown));
    *lock(&shared.picker) = Some(MainThreadBound::new(picker.downgrade(), mtm));
    // UIKit refuses a presentation (logging a warning, never calling back)
    // when the presenter is not in a window or is mid-transition, so no
    // `didFinishPicking:` would ever arrive: give up once the grace period
    // passes without the completion block.
    let watch = MainThreadBound::new((picker, shown, tx.clone()), mtm);
    let when = DispatchTime::try_from(PRESENT_GRACE).unwrap_or(DispatchTime::NOW);
    let _ = DispatchQueue::main().after(when, move || {
        // SAFETY: the block is submitted to the main queue, which always runs
        // on the process's main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let (picker, shown, tx) = watch.get(mtm);
        if shown.get() == Shown::Pending {
            shown.set(Shown::GaveUp);
            picker.abandon();
            let _ = tx.send(Event::Failed(PhotoError::NoPresenter));
        }
    });
    Ok(())
}

/// Wait for the outcome of one pick (module doc's *Threading* and *Results*),
/// reporting the loading phase through `progress`. `load_fraction` reads the
/// started load for an index; `cancelled` is checked on every wake-up. The cancel
/// path holds no sender, so a load that never answers still ends the wait when
/// its handler is released.
fn collect(
    rx: &Receiver<Event>,
    progress: Option<&ProgressThrottle>,
    load_fraction: &dyn Fn(usize) -> Option<f64>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<PickedItem>, PhotoError> {
    let count = loop {
        let event = rx.recv_timeout(POLL);
        if cancelled() {
            return Err(PhotoError::Cancelled);
        }
        match event {
            Ok(Event::Failed(e)) => return Err(e),
            Ok(Event::Finished(count)) => break count,
            Ok(Event::Item(..)) | Err(RecvTimeoutError::Timeout) => continue,
            // Every sender is gone without an answer: a dismissal.
            Err(RecvTimeoutError::Disconnected) => return Ok(Vec::new()),
        }
    };
    if count == 0 {
        return Ok(Vec::new());
    }
    let report = |done: usize, fraction: f64| {
        if let Some(throttle) = progress {
            throttle.report(PickProgress {
                items: count,
                done,
                fraction,
            });
        }
    };
    report(0, 0.0);
    let mut slots: Vec<Option<PickedItem>> = (0..count).map(|_| None).collect();
    let mut answered = vec![false; count];
    let mut done = 0;
    while done < count {
        let event = rx.recv_timeout(POLL);
        if cancelled() {
            return Err(PhotoError::Cancelled);
        }
        match event {
            Ok(Event::Item(index, outcome)) => {
                done += 1;
                if let Some(flag) = answered.get_mut(index) {
                    *flag = true;
                }
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
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            // Every handler is gone; whatever did not answer is lost.
            Err(RecvTimeoutError::Disconnected) => {
                log::warn!(
                    "photo picker: {} of {count} items never answered",
                    count - done
                );
                break;
            }
        }
        // The final report waits for a successful outcome, below.
        if done < count {
            report(done, overall_fraction(&answered, load_fraction));
        }
    }
    let items: Vec<PickedItem> = slots.into_iter().flatten().collect();
    if items.is_empty() {
        return Err(PhotoError::Platform(format!(
            "none of the {count} picked items could be copied"
        )));
    }
    report(count, 1.0);
    Ok(items)
}

/// The mean progress over every item: `1.0` once answered, otherwise its
/// load's fraction (`0.0` before the load starts), each clamped to `0.0..=1.0`.
fn overall_fraction(answered: &[bool], load_fraction: &dyn Fn(usize) -> Option<f64>) -> f64 {
    if answered.is_empty() {
        return 1.0;
    }
    let sum: f64 = answered
        .iter()
        .enumerate()
        .map(|(index, &answered)| {
            if answered {
                return 1.0;
            }
            match load_fraction(index) {
                Some(f) if !f.is_nan() => f.clamp(0.0, 1.0),
                _ => 0.0,
            }
        })
        .sum();
    sum / answered.len() as f64
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
    use std::sync::atomic::{AtomicU64, AtomicUsize};

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

    /// `collect` with no progress callback and no started loads.
    fn collect_quiet(rx: &Receiver<Event>) -> Result<Vec<PickedItem>, PhotoError> {
        collect(rx, None, &|_| None, &|| false)
    }

    type Log = Arc<Mutex<Vec<PickProgress>>>;

    /// A throttle whose clock jumps a second per call, so every report lands.
    fn unthrottled() -> (ProgressThrottle, Log) {
        let log: Log = Arc::default();
        let calls = Arc::new(AtomicU64::new(0));
        let sink = Arc::clone(&log);
        let throttle = ProgressThrottle::with_clock(
            Arc::new(move |p| sink.lock().unwrap().push(p)),
            Box::new(move || Duration::from_secs(calls.fetch_add(1, Ordering::SeqCst))),
        );
        (throttle, log)
    }

    fn p(items: usize, done: usize, fraction: f64) -> PickProgress {
        PickProgress {
            items,
            done,
            fraction,
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
        assert_eq!(names(&collect_quiet(&rx).unwrap()), ["a", "b", "c"]);
    }

    #[test]
    fn a_failed_item_is_skipped() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Finished(2)).unwrap();
        tx.send(Event::Item(0, Err("load failed".to_owned())))
            .unwrap();
        tx.send(Event::Item(1, Ok(item("b")))).unwrap();
        assert_eq!(names(&collect_quiet(&rx).unwrap()), ["b"]);
    }

    #[test]
    fn all_failed_is_a_platform_error() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Finished(1)).unwrap();
        tx.send(Event::Item(0, Err("load failed".to_owned())))
            .unwrap();
        assert!(matches!(collect_quiet(&rx), Err(PhotoError::Platform(_))));
    }

    #[test]
    fn cancel_and_vanished_picker_are_empty() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Finished(0)).unwrap();
        assert!(collect_quiet(&rx).unwrap().is_empty());
        let (tx, rx) = mpsc::channel::<Event>();
        drop(tx);
        assert!(collect_quiet(&rx).unwrap().is_empty());
    }

    #[test]
    fn a_presentation_failure_is_returned() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Failed(PhotoError::NoPresenter)).unwrap();
        assert_eq!(collect_quiet(&rx).unwrap_err(), PhotoError::NoPresenter);
    }

    #[test]
    fn lost_handlers_keep_what_arrived() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Finished(2)).unwrap();
        tx.send(Event::Item(1, Ok(item("b")))).unwrap();
        drop(tx);
        assert_eq!(names(&collect_quiet(&rx).unwrap()), ["b"]);
    }

    #[test]
    fn cancel_while_the_picker_is_open() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Finished(1)).unwrap();
        let outcome = collect(&rx, None, &|_| None, &|| true);
        assert_eq!(outcome.unwrap_err(), PhotoError::Cancelled);
    }

    #[test]
    fn cancel_mid_load_drops_what_arrived() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Finished(2)).unwrap();
        tx.send(Event::Item(0, Ok(item("a")))).unwrap();
        tx.send(Event::Item(1, Ok(item("b")))).unwrap();
        // Cancelled on the wake-up after item 0 (Finished, item 0, then this).
        let wakes = AtomicUsize::new(0);
        let cancelled = || wakes.fetch_add(1, Ordering::SeqCst) >= 2;
        let outcome = collect(&rx, None, &|_| None, &cancelled);
        assert_eq!(outcome.unwrap_err(), PhotoError::Cancelled);
    }

    #[test]
    fn a_cancel_that_races_the_last_answer_still_wins() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Finished(1)).unwrap();
        tx.send(Event::Item(0, Err("cancelled".to_owned())))
            .unwrap();
        let wakes = AtomicUsize::new(0);
        let cancelled = || wakes.fetch_add(1, Ordering::SeqCst) >= 1;
        let outcome = collect(&rx, None, &|_| None, &cancelled);
        assert_eq!(outcome.unwrap_err(), PhotoError::Cancelled);
    }

    #[test]
    fn a_failed_pick_never_reports_completion() {
        let (throttle, log) = unthrottled();
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Finished(1)).unwrap();
        tx.send(Event::Item(0, Err("copy failed".to_owned())))
            .unwrap();
        let outcome = collect(&rx, Some(&throttle), &|_| None, &|| false);
        assert!(matches!(outcome, Err(PhotoError::Platform(_))));
        assert!(log.lock().unwrap().iter().all(|p| p.fraction < 1.0));
    }

    #[test]
    fn progress_starts_at_zero_and_ends_at_one() {
        let (throttle, log) = unthrottled();
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Finished(2)).unwrap();
        tx.send(Event::Item(0, Ok(item("a")))).unwrap();
        tx.send(Event::Item(1, Ok(item("b")))).unwrap();
        // Item 1's load is half done when item 0 answers.
        let source = |index: usize| (index == 1).then_some(0.5);
        assert_eq!(
            collect(&rx, Some(&throttle), &source, &|| false)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            *log.lock().unwrap(),
            [p(2, 0, 0.0), p(2, 1, 0.75), p(2, 2, 1.0)]
        );
    }

    #[test]
    fn polling_reports_load_fractions_between_items() {
        let (throttle, log) = unthrottled();
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Finished(1)).unwrap();
        let reads = Arc::new(AtomicUsize::new(0));
        let source = {
            let reads = Arc::clone(&reads);
            move |_: usize| {
                reads.fetch_add(1, Ordering::SeqCst);
                Some(0.4)
            }
        };
        // Answer only after a poll tick has read the fake source.
        let answer = std::thread::spawn(move || {
            while reads.load(Ordering::SeqCst) == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
            tx.send(Event::Item(0, Ok(item("a")))).unwrap();
        });
        assert_eq!(
            collect(&rx, Some(&throttle), &source, &|| false)
                .unwrap()
                .len(),
            1
        );
        answer.join().unwrap();
        let log = log.lock().unwrap();
        assert_eq!(log.first(), Some(&p(1, 0, 0.0)));
        assert!(log.contains(&p(1, 0, 0.4)), "{log:?}");
        assert_eq!(log.last(), Some(&p(1, 1, 1.0)));
        assert!(log.windows(2).all(|w| w[0].fraction <= w[1].fraction));
    }

    #[test]
    fn overall_fraction_is_the_mean_of_items() {
        let none = |_: usize| None;
        assert_eq!(overall_fraction(&[], &none), 1.0);
        assert_eq!(overall_fraction(&[false, false], &none), 0.0);
        assert_eq!(overall_fraction(&[true, false], &none), 0.5);
        let odd = |index: usize| [Some(f64::NAN), Some(7.0), Some(-1.0), Some(0.5)][index];
        assert_eq!(overall_fraction(&[false, false, false, false], &odd), 0.375);
        assert_eq!(overall_fraction(&[true, true, true, true], &odd), 1.0);
    }
}
