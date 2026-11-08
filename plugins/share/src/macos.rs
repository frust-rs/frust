//! The macOS file-share backend: `NSSharingServicePicker` shown relative to
//! the key window's content view.
//!
//! macOS text/URL shares keep the clipboard path (`desktop.rs`); only files
//! come here.
//!
//! # Main-thread dispatch
//!
//! AppKit views are `MainThreadOnly`, so [`share_files`] enqueues the picker
//! on the main queue and returns `Ok(Presented)`. When already on the main
//! thread the window check runs synchronously and a missing window surfaces
//! as [`ShareError::NoPresenter`]; from any other thread a miss is dropped.
//!
//! # Anchor
//!
//! A [`ShareAnchor`] is in logical points from the top-left of the content
//! view. AppKit views are bottom-left unless `isFlipped`, so [`view_rect`]
//! flips the y axis for unflipped views. Without an anchor the picker opens
//! at a zero-size rect at the view's centre.

use std::path::PathBuf;

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSSharingServicePicker, NSView};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSRectEdge, NSSize, NSString, NSURL};

use crate::{ShareAnchor, ShareError, ShareOutcome};

/// Enqueue the picker on the main queue (module doc's *Main-thread dispatch*).
///
/// # Errors
/// [`ShareError::NoPresenter`] when called on the main thread and the app has
/// no window with a content view.
pub(crate) fn share_files(
    paths: Vec<PathBuf>,
    anchor: Option<&ShareAnchor>,
) -> Result<ShareOutcome, ShareError> {
    if let Some(mtm) = MainThreadMarker::new()
        && content_view(mtm).is_none()
    {
        return Err(ShareError::NoPresenter);
    }
    let anchor = anchor.copied();
    DispatchQueue::main().exec_async(move || {
        // SAFETY: this closure is submitted to `dispatch_get_main_queue()`,
        // which always executes its blocks on the process's main thread, so
        // constructing a `MainThreadMarker` here without re-checking is sound.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        present_on_main(mtm, &paths, anchor.as_ref());
    });
    Ok(ShareOutcome::Presented)
}

/// Build and show the picker; a miss is dropped (no caller is left).
fn present_on_main(mtm: MainThreadMarker, paths: &[PathBuf], anchor: Option<&ShareAnchor>) {
    let Some(view) = content_view(mtm) else {
        return;
    };
    let urls: Vec<Retained<AnyObject>> = paths
        .iter()
        .filter_map(|p| p.to_str())
        .map(|p| Retained::<AnyObject>::from(NSURL::fileURLWithPath(&NSString::from_str(p))))
        .collect();
    if urls.is_empty() {
        return;
    }
    let items = NSArray::<AnyObject>::from_retained_slice(&urls);
    // SAFETY: `items` is an `NSArray` of `NSURL`, which conforms to
    // `NSPasteboardWriting`, the item type the binding requires.
    let picker =
        unsafe { NSSharingServicePicker::initWithItems(NSSharingServicePicker::alloc(), &items) };
    let bounds = view.bounds();
    let rect = view_rect(
        anchor.map(|a| a.rect),
        (bounds.size.width, bounds.size.height),
        view.isFlipped(),
    );
    picker.showRelativeToRect_ofView_preferredEdge(
        NSRect::new(
            NSPoint::new(rect.x0, rect.y0),
            NSSize::new(rect.width(), rect.height()),
        ),
        &view,
        NSRectEdge::MinY,
    );
}

/// The key window's content view, falling back to the main window's.
fn content_view(mtm: MainThreadMarker) -> Option<Retained<NSView>> {
    let app = NSApplication::sharedApplication(mtm);
    app.keyWindow()
        .and_then(|w| w.contentView())
        .or_else(|| app.mainWindow().and_then(|w| w.contentView()))
}

/// Map an optional top-left anchor to the view's coordinate space within a
/// view of `(width, height)`; a missing or non-finite anchor becomes a
/// zero-size rect at the centre.
fn view_rect(anchor: Option<kurbo::Rect>, size: (f64, f64), flipped: bool) -> kurbo::Rect {
    match anchor {
        Some(rect) if rect.is_finite() => {
            let r = rect.abs();
            if flipped {
                r
            } else {
                kurbo::Rect::new(r.x0, size.1 - r.y1, r.x1, size.1 - r.y0)
            }
        }
        _ => kurbo::Rect::from_center_size((size.0 / 2.0, size.1 / 2.0), (0.0, 0.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_rect_flips_for_unflipped_views_only() {
        let a = kurbo::Rect::new(10.0, 20.0, 50.0, 60.0);
        assert_eq!(view_rect(Some(a), (100.0, 200.0), true), a);
        assert_eq!(
            view_rect(Some(a), (100.0, 200.0), false),
            kurbo::Rect::new(10.0, 140.0, 50.0, 180.0)
        );
    }

    #[test]
    fn view_rect_centres_without_a_usable_anchor() {
        let centre = kurbo::Rect::new(50.0, 100.0, 50.0, 100.0);
        assert_eq!(view_rect(None, (100.0, 200.0), false), centre);
        let bad = kurbo::Rect::new(f64::NAN, 0.0, 1.0, 1.0);
        assert_eq!(view_rect(Some(bad), (100.0, 200.0), true), centre);
    }
}
