//! `frust-photo-library`: pick photos and videos with the platform's own picker.
//!
//! - **Android, iOS:** the system photo picker (no photo-library permission needed to pick).
//! - **Linux, macOS, Windows:** a native file dialog through `frust-file-dialogs`, filtered to
//!   image and video extensions.
//!
//! # Charter: a platform plugin
//!
//! Like `frust-permissions`, this is a **platform plugin**: an app adds it to its own
//! `Cargo.toml` alongside `frust`; the `frust` facade does not re-export it. Its only
//! `frust-*` dependencies are `frust-paths` (the pick cache location), `frust-file-dialogs`
//! on desktop and `frust-plugin` on Android.
//!
//! # Threading
//!
//! [`PhotoLibrary::pick`] blocks until the user chooses or cancels. On the platform UI thread
//! of a mobile target it returns [`PhotoError::UiThread`]; pair it with `frust::spawn_blocking`.
//!
//! # Cancellation and copies
//!
//! An empty `Vec` means the user cancelled. On mobile each pick is **copied** into an app-private
//! cache (`PickedItem::is_copy` is `true`) so the path stays readable after the picker closes;
//! call [`PhotoLibrary::clear_pick_cache`] to reclaim the space. Desktop returns the user's
//! original files (`is_copy` is `false`) and never copies.
//!
//! ```no_run
//! use frust_photo_library::{
//!     CancelToken, MediaFilter, PhotoError, PhotoLibrary, PickOptions, PickProgress,
//! };
//!
//! let cancel = CancelToken::new();
//! let opts = PickOptions::new()
//!     .filter(MediaFilter::Videos)
//!     .on_progress(move |p: PickProgress| {
//!         let _ = (p.items, p.done, p.fraction);
//!     })
//!     .cancel(cancel.clone());
//! // UI: cancel.cancel();
//!
//! // Blocks: call inside `frust::spawn_blocking`.
//! match PhotoLibrary::pick(&opts) {
//!     Err(PhotoError::Cancelled) => {}
//!     Ok(items) => {
//!         for item in items {
//!             let _ = item.path; // decode with `frust::decode_image_async`
//!         }
//!     }
//!     Err(_) => {}
//! }
//! ```

mod cancel;
mod types;

// Desktop uses only part of these; mobile targets keep the dead-code lint.
#[cfg_attr(not(any(target_os = "android", target_os = "ios")), allow(dead_code))]
mod media_types;
#[cfg_attr(not(any(target_os = "android", target_os = "ios")), allow(dead_code))]
mod pick_cache;

pub use cancel::CancelToken;
pub use types::{
    AssetId, MediaFilter, MediaKind, PhotoError, PickOptions, PickProgress, PickedItem,
};

#[cfg(target_os = "ios")]
mod apple;
#[cfg(target_os = "ios")]
use apple as platform;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
use android as platform;

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
mod desktop;
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
use desktop as platform;

#[cfg(not(any(
    target_os = "ios",
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
)))]
mod unsupported;
#[cfg(not(any(
    target_os = "ios",
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
)))]
use unsupported as platform;

/// Entry point for picking media.
pub struct PhotoLibrary;

impl PhotoLibrary {
    /// Shows the platform picker and waits. Blocks; see the crate-level threading notes.
    /// An empty `Vec` means the user dismissed the picker.
    ///
    /// # Loading and cancel
    ///
    /// After the picker closes, the chosen items may still be loading (an iCloud video or a
    /// fresh photo can take seconds to minutes). [`PickOptions::on_progress`] reports that
    /// phase: its first call is the loading signal, and calls are rate-limited. Pass a
    /// [`CancelToken`] through [`PickOptions::cancel`] to abort from another thread.
    /// Cancelling while items load returns `Err(`[`PhotoError::Cancelled`]`)` and the call's
    /// partial copies are removed. Cancelling while the picker is open returns the same error
    /// at once on iOS (which dismisses the picker) and on Android (where the system picker
    /// stays on screen until the user closes it, and its result is discarded). On desktop the
    /// call returns only after the file dialog closes.
    pub fn pick(options: &PickOptions) -> Result<Vec<PickedItem>, PhotoError> {
        platform::pick(options)
    }

    /// Deletes every file copied by earlier mobile picks. A missing cache is success.
    ///
    /// Returns [`PhotoError::Busy`] while a mobile pick is in flight (and a pick cannot start
    /// while the clear runs). Paths from earlier picks' [`PickedItem`]s become invalid after a
    /// clear; the caller owns that.
    pub fn clear_pick_cache() -> Result<(), PhotoError> {
        pick_cache::clear()
    }
}
