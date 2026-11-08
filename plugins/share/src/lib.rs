//! `frust-share`: a platform plugin that hands text, an http/https URL or
//! files to the platform share sheet — Android `ACTION_SEND` chooser via
//! plain JNI, iOS `UIActivityViewController` activity sheet, macOS
//! `NSSharingServicePicker` (files), desktop clipboard copy as the fallback
//! for text and URLs. Files are `Unsupported` on Linux and Windows.
//!
//! # Charter: a platform plugin
//!
//! Depends on `frust-plugin` plus FFI crates; the one other `frust-*` edge is
//! `frust-clipboard` on desktop only (see `Cargo.toml`). An app adds this
//! crate to its own `Cargo.toml` alongside `frust`.
//!
//! # Fire-and-forget
//!
//! `Ok` means the platform call was enqueued; what the user then picks in the
//! sheet is unobservable. No `Display` message echoes the shared payload.

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod apple;
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
mod desktop;
mod files;
#[cfg(target_os = "macos")]
mod macos;
mod url;

use std::path::{Path, PathBuf};

/// Where an iOS popover should anchor, in logical points (ignored on other
/// platforms; accepted everywhere so the signature is final).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShareAnchor {
    /// The anchor rectangle, logical points.
    pub rect: kurbo::Rect,
}

/// What a successful [`Share`] call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShareOutcome {
    /// The platform share sheet was presented (or enqueued for presentation).
    Presented,
    /// No share sheet exists here; the payload was copied to the clipboard.
    CopiedToClipboard,
}

/// Errors from a [`Share`] call. No variant's message echoes the payload.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum ShareError {
    /// The text or URL was empty.
    #[error("share: empty payload")]
    EmptyPayload,

    /// `Share::url` was given something other than an absolute http/https URL.
    #[error("share: not an absolute http/https URL")]
    InvalidUrl,

    /// The Android host shell never installed the `(JavaVM, Context)` handles.
    #[error("share platform not initialized")]
    PlatformNotInitialized,

    /// No view controller / activity is available to present from.
    #[error("share: no presenter available")]
    NoPresenter,

    /// `Share::files` was given more than 64 files.
    #[error("share: too many files")]
    TooManyFiles,

    /// A file path was relative, missing, not a regular file, or not UTF-8.
    #[error("share: invalid file")]
    InvalidFile,

    /// This platform cannot share the payload (files on Linux, Windows, web).
    #[error("share: unsupported on this platform")]
    Unsupported,

    /// A backend failure (JNI exception class name, clipboard error kind, …).
    #[error("share error: {0}")]
    Platform(String),
}

/// The platform share entry point; carries no state.
pub struct Share;

impl Share {
    /// Share `text`, with an optional `subject` (used where the platform has one).
    ///
    /// # Errors
    /// [`ShareError::EmptyPayload`] for empty `text`; otherwise per platform.
    pub fn text(text: &str, subject: Option<&str>) -> Result<ShareOutcome, ShareError> {
        Self::text_anchored(text, subject, None)
    }

    /// As [`Self::text`], with an iOS popover `anchor` (ignored elsewhere).
    ///
    /// # Errors
    /// See [`Self::text`].
    pub fn text_anchored(
        text: &str,
        subject: Option<&str>,
        anchor: Option<&ShareAnchor>,
    ) -> Result<ShareOutcome, ShareError> {
        if text.is_empty() {
            return Err(ShareError::EmptyPayload);
        }
        dispatch(text, PayloadKind::Text, subject, anchor)
    }

    /// Share an absolute http/https `url`.
    ///
    /// # Errors
    /// [`ShareError::EmptyPayload`] for an empty `url`, [`ShareError::InvalidUrl`]
    /// if it fails the http/https validator; otherwise per platform.
    pub fn url(url: &str, subject: Option<&str>) -> Result<ShareOutcome, ShareError> {
        if url.is_empty() {
            return Err(ShareError::EmptyPayload);
        }
        crate::url::validate(url)?;
        dispatch(url, PayloadKind::Url, subject, None)
    }

    /// Share one or more files (at most 64) through the platform share UI.
    ///
    /// Each path must be absolute and name an existing regular file; paths
    /// are canonicalised before dispatch.
    ///
    /// # Threading
    /// Never blocks on file size on any platform, so it may be called from a
    /// UI handler. On Android `Ok(ShareOutcome::Presented)` means the share
    /// was enqueued: staging happens on a background thread, and a later
    /// staging failure is logged and shows no chooser.
    ///
    /// # Errors
    /// [`ShareError::EmptyPayload`] for no paths, [`ShareError::TooManyFiles`]
    /// above 64, [`ShareError::InvalidFile`] for a bad path,
    /// [`ShareError::Unsupported`] where files cannot be shared.
    pub fn files(paths: &[&Path], subject: Option<&str>) -> Result<ShareOutcome, ShareError> {
        Self::files_anchored(paths, subject, None)
    }

    /// As [`Self::files`], with a popover `anchor` (iOS iPad and the macOS
    /// picker; ignored elsewhere).
    ///
    /// # Threading
    /// As [`Self::files`]: never blocks on file size; on Android `Presented`
    /// means enqueued and a later staging failure is logged.
    ///
    /// # Errors
    /// See [`Self::files`].
    pub fn files_anchored(
        paths: &[&Path],
        subject: Option<&str>,
        anchor: Option<&ShareAnchor>,
    ) -> Result<ShareOutcome, ShareError> {
        let paths = crate::files::validate(paths)?;
        dispatch_files(paths, subject, anchor)
    }
}

/// Which `Share` entry point a payload came from (iOS hands a URL over as an
/// `NSURL`; the other backends treat both as text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PayloadKind {
    Text,
    Url,
}

/// An iPad popover's source: `rect` in the root view's space, and whether
/// UIKit may draw an arrow to it.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
struct PopoverSource {
    rect: kurbo::Rect,
    arrows: bool,
}

/// Convert an optional [`ShareAnchor`] into the popover source within a root
/// view whose bounds are `bounds`. Logical points are UIKit points (1:1), so
/// the anchor is only normalised and offset by the bounds origin; a missing
/// or non-finite anchor becomes a zero-size rect at the bounds' centre with
/// no arrow.
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn popover_source(anchor: Option<&ShareAnchor>, bounds: kurbo::Rect) -> PopoverSource {
    match anchor.map(|a| a.rect) {
        Some(rect) if rect.is_finite() => PopoverSource {
            rect: rect.abs() + bounds.origin().to_vec2(),
            arrows: true,
        },
        _ => PopoverSource {
            rect: kurbo::Rect::from_center_size(bounds.center(), (0.0, 0.0)),
            arrows: false,
        },
    }
}

fn dispatch(
    text: &str,
    kind: PayloadKind,
    subject: Option<&str>,
    anchor: Option<&ShareAnchor>,
) -> Result<ShareOutcome, ShareError> {
    #[cfg(target_os = "ios")]
    {
        apple::share(text, kind, subject, anchor)
    }
    #[cfg(target_os = "android")]
    {
        let _ = (kind, anchor);
        android::share(text, subject)
    }
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        let _ = (kind, subject, anchor);
        desktop::share(text)
    }
    #[cfg(not(any(
        target_os = "ios",
        target_os = "android",
        target_os = "macos",
        target_os = "linux",
        target_os = "windows"
    )))]
    {
        let _ = (text, kind, subject, anchor);
        Err(ShareError::Platform("unsupported platform".to_string()))
    }
}

/// Route validated, canonical `paths` to the platform's file-share backend.
fn dispatch_files(
    paths: Vec<PathBuf>,
    subject: Option<&str>,
    anchor: Option<&ShareAnchor>,
) -> Result<ShareOutcome, ShareError> {
    #[cfg(target_os = "ios")]
    {
        apple::share_files(paths, subject, anchor)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = subject;
        macos::share_files(paths, anchor)
    }
    #[cfg(target_os = "android")]
    {
        let _ = anchor;
        android::share_files(&paths, subject)
    }
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        let _ = (paths, subject, anchor);
        desktop::share_files()
    }
    #[cfg(not(any(
        target_os = "ios",
        target_os = "android",
        target_os = "macos",
        target_os = "linux",
        target_os = "windows"
    )))]
    {
        let _ = (paths, subject, anchor);
        Err(ShareError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_payload_is_refused() {
        assert!(matches!(
            Share::text("", None),
            Err(ShareError::EmptyPayload)
        ));
        assert!(matches!(
            Share::url("", None),
            Err(ShareError::EmptyPayload)
        ));
    }

    #[test]
    fn url_refuses_unsafe_schemes() {
        for u in ["javascript:alert(1)", "file:///etc/passwd", "tel:+1"] {
            assert!(matches!(Share::url(u, None), Err(ShareError::InvalidUrl)));
        }
    }

    #[test]
    fn error_display_never_echoes_payload() {
        const MARKER: &str = "marker-payload";
        for e in [
            ShareError::EmptyPayload,
            ShareError::InvalidUrl,
            ShareError::PlatformNotInitialized,
            ShareError::NoPresenter,
            ShareError::TooManyFiles,
            ShareError::InvalidFile,
            ShareError::Unsupported,
        ] {
            assert!(!e.to_string().contains(MARKER));
        }
    }

    #[test]
    fn popover_source_maps_anchor_one_to_one_or_centres() {
        let bounds = kurbo::Rect::new(0.0, 0.0, 400.0, 800.0);
        let anchor = ShareAnchor {
            rect: kurbo::Rect::new(120.0, 40.0, 20.0, 10.0),
        };
        let anchored = popover_source(Some(&anchor), bounds);
        assert_eq!(anchored.rect, kurbo::Rect::new(20.0, 10.0, 120.0, 40.0));
        assert!(anchored.arrows);

        let offset = popover_source(Some(&anchor), bounds + kurbo::Vec2::new(5.0, 7.0));
        assert_eq!(offset.rect, kurbo::Rect::new(25.0, 17.0, 125.0, 47.0));

        let centred = popover_source(None, bounds);
        assert_eq!(centred.rect, kurbo::Rect::new(200.0, 400.0, 200.0, 400.0));
        assert!(!centred.arrows);

        let bad = ShareAnchor {
            rect: kurbo::Rect::new(f64::NAN, 0.0, 1.0, 1.0),
        };
        assert_eq!(popover_source(Some(&bad), bounds), centred);
    }

    #[test]
    fn files_refuses_bad_input_before_dispatch() {
        assert!(matches!(
            Share::files(&[], None),
            Err(ShareError::EmptyPayload)
        ));
        assert!(matches!(
            Share::files(&[Path::new("relative.bin")], None),
            Err(ShareError::InvalidFile)
        ));
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    fn files_are_unsupported_on_linux_and_windows() {
        let file =
            std::env::temp_dir().join(format!("frust-share-unsupported-{}", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let result = Share::files(&[file.as_path()], None);
        std::fs::remove_file(&file).ok();
        assert!(matches!(result, Err(ShareError::Unsupported)));
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    #[test]
    #[ignore = "writes the host clipboard; run explicitly with -- --ignored"]
    fn desktop_falls_back_to_clipboard_or_reports_platform() {
        match Share::text("frust-share test", None) {
            Ok(outcome) => assert_eq!(outcome, ShareOutcome::CopiedToClipboard),
            Err(e) => assert!(matches!(e, ShareError::Platform(_))),
        }
    }
}
