//! The UIKit half of [`frust_reactive::BackgroundTask`]:
//! `beginBackgroundTask`/`endBackgroundTask` driven from Rust.
//!
//! # Threads
//!
//! `UIApplication` is `MainThreadOnly`, so reaching it takes a [`MainThreadMarker`].
//! `begin` runs on the UI thread, which on iOS is the main thread; if it ever does not,
//! it reports the task refused rather than calling UIKit from the wrong thread. `end` is
//! the opposite case: the guard is normally dropped on a `spawn_blocking` thread once the
//! write finishes, so `end` hands the release to the main queue. The hand-off is
//! asynchronous because a blocking bounce would stall the writer on a main thread that may
//! be busy; the assertion stays held until that block runs, so the process cannot suspend
//! while the release is in flight.
//!
//! # Expiry
//!
//! Every begin must be balanced by exactly one end or the system kills the app. When time
//! runs out UIKit calls the expiration handler synchronously on the main thread, and the
//! handler must end the task and return quickly. It records the expiry first, so a writer
//! polling [`BackgroundTask::expired`](frust_reactive::BackgroundTask::expired) sees it,
//! then ends the task only if [`TaskState::mark_ended`] grants it the right. The guard's
//! `Drop` calls `mark_ended` before `end`, so the handler and the drop path never both end
//! one assertion.
//!
//! An expiry that arrives after `Drop` has claimed `ended` is skipped by the handler,
//! while the main-queue release `Drop` queued ends the task one turn later. That is safe:
//! UIKit tolerates it. The shell installs no backend of its own; this plugin does.
//!
//! An id of `0` means no assertion was taken (UIKit refused, or `begin` ran off the main
//! thread), and `end` does nothing for it.
//!
//!
//! The shell's own font-cache flush on entering the background (`BACKGROUND_FLUSH` in
//! the iOS shell's `app::executor`) takes no assertion.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use block2::RcBlock;
use dispatch2::DispatchQueue;
use frust_reactive::{BackgroundTaskBackend, TaskState};
use objc2::MainThreadMarker;
use objc2_foundation::NSString;
use objc2_ui_kit::{UIApplication, UIBackgroundTaskIdentifier, UIBackgroundTaskInvalid};

/// The backend [`install`] registers. Stateless: each task's state travels in its
/// `TaskState` and its expiration block.
pub(crate) struct IosBackgroundTasks;

impl BackgroundTaskBackend for IosBackgroundTasks {
    fn begin(&self, name: &str, state: &Arc<TaskState>) -> u64 {
        let Some(mtm) = MainThreadMarker::new() else {
            state.set_refused();
            return 0;
        };

        // Written after UIKit returns the identifier. The block cannot read it earlier:
        // UIKit runs the block on the main thread, which is busy here until `begin` returns.
        let identifier = Arc::new(AtomicU64::new(0));
        let expiration_handler = {
            let state = Arc::clone(state);
            let identifier = Arc::clone(&identifier);
            RcBlock::new(move || {
                state.set_expired();
                if state.mark_ended() {
                    end_on_main(identifier.load(Ordering::SeqCst) as UIBackgroundTaskIdentifier);
                }
            })
        };

        let id = UIApplication::sharedApplication(mtm)
            .beginBackgroundTaskWithName_expirationHandler(
                Some(&NSString::from_str(name)),
                Some(&expiration_handler),
            );

        // SAFETY: `UIBackgroundTaskInvalid` is a constant UIKit exports and never writes,
        // so this plain read of a `usize` cannot race.
        if id == unsafe { UIBackgroundTaskInvalid } {
            state.set_refused();
            return 0;
        }

        identifier.store(id as u64, Ordering::SeqCst);
        id as u64
    }

    fn end(&self, id: u64, _state: &Arc<TaskState>) {
        if id == 0 {
            return;
        }
        end_on_main(id as UIBackgroundTaskIdentifier);
    }
}

/// Release `id` on the main thread: directly when already there, otherwise through the main
/// queue. The expiration handler also comes through here, so a release is never dropped
/// even if UIKit ran that handler off the main thread against its documented behaviour.
fn end_on_main(id: UIBackgroundTaskIdentifier) {
    if let Some(mtm) = MainThreadMarker::new() {
        UIApplication::sharedApplication(mtm).endBackgroundTask(id);
        return;
    }
    DispatchQueue::main().exec_async(move || {
        if let Some(mtm) = MainThreadMarker::new() {
            UIApplication::sharedApplication(mtm).endBackgroundTask(id);
        }
    });
}

/// Register the UIKit backend if none is installed yet; an app-installed backend is kept.
pub(crate) fn install() -> bool {
    frust_reactive::install_background_task_backend(Arc::new(IosBackgroundTasks))
}
