//! `frust-share`: a platform plugin that hands text or an http/https URL to
//! the platform share sheet — Android `ACTION_SEND` chooser via plain JNI,
//! iOS `UIActivityViewController` activity sheet, desktop clipboard copy as
//! the fallback.
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
mod url;

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

#[allow(unused_variables)]
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
        android::share(text, subject)
    }
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
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
        Err(ShareError::Platform("unsupported platform".to_string()))
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

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    #[test]
    fn desktop_falls_back_to_clipboard_or_reports_platform() {
        match Share::text("frust-share test", None) {
            Ok(outcome) => assert_eq!(outcome, ShareOutcome::CopiedToClipboard),
            Err(e) => assert!(matches!(e, ShareError::Platform(_))),
        }
    }
}
