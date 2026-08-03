//! The iOS [`Backend`] — `UIPasteboard` via `objc2-ui-kit`.
//!
//! `#[cfg(target_os = "ios")]` only — **not** `target_vendor = "apple"`:
//! `UIPasteboard` is UIKit, which doesn't exist on macOS. macOS shares
//! [`crate::desktop`] (`arboard`, whose own macOS backend drives
//! `NSPasteboard` via `objc2-app-kit`) instead — see this crate's
//! `Cargo.toml` comment for the same rationale `plugins/native-widgets`
//! already documents for its own iOS-only UIKit arm.
//!
//! # Thread-safety
//!
//! `UIPasteboard` carries **no documented main-thread requirement** — unlike
//! most UIKit types it is not `@MainActor`-isolated, and Apple's own
//! documentation describes `UIPasteboard.general` as safe to use from any
//! thread. `objc2-ui-kit`'s generated bindings mark `string`/`setString:`
//! `unsafe` only because the header's "This might not be thread-safe" note
//! is conservative boilerplate (header-translator's default for any
//! non-atomic Objective-C property), not a documented hazard — this backend
//! calls them directly from the caller's thread, each call `# Safety`-noted
//! below per `docs/CODE_STANDARDS.md`'s sanctioned-unsafe convention.
//!
//! # iOS 14+ paste banner
//!
//! Reading the general pasteboard (`string`) on iOS 14+ shows a one-time
//! system banner ("App pasted from Notes", etc.) the first time an app reads
//! pasteboard content after gaining focus — a privacy notice, not an error
//! and not something this backend can suppress. `UIPasteboard`'s
//! `detectPatterns`/`detectValues` APIs exist to probe pasteboard content
//! *without* triggering the banner, but are out of scope for this crate's
//! plain `get_text`/`set_text` API.
//!
//! # `unsafe`
//!
//! Confined to this module, mirroring `frust-shared-preferences`'s `apple`
//! backend precedent: `UIPasteboard::string`/`setString:` are the only two
//! calls objc2-ui-kit marks `unsafe`, each `# Safety`-noted. This backend
//! holds no Objective-C reference across calls (it re-fetches
//! `generalPasteboard` per operation, a cheap singleton lookup), so
//! [`AppleClipboard`] is a plain value, trivially `Send + Sync`.

use objc2_foundation::NSString;
use objc2_ui_kit::UIPasteboard;

use crate::{Backend, ClipboardError};

/// The iOS backend. Stateless — every operation re-fetches
/// `UIPasteboard.generalPasteboard`, a cheap singleton lookup.
pub(crate) struct AppleClipboard;

impl Backend for AppleClipboard {
    fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        let pasteboard = UIPasteboard::generalPasteboard();
        let value = NSString::from_str(text);
        // SAFETY: `setString:` is a plain property write with a valid
        // `NSString` argument; `UIPasteboard` is documented callable from
        // any thread (see the module doc's *Thread-safety*).
        unsafe { pasteboard.setString(Some(&value)) };
        Ok(())
    }

    fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        let pasteboard = UIPasteboard::generalPasteboard();
        // SAFETY: `string` is a plain property read; `UIPasteboard` is
        // documented callable from any thread (see the module doc's
        // *Thread-safety*). Reading here may show the iOS 14+ paste banner
        // (module doc) — expected, not an error.
        let value = unsafe { pasteboard.string() };
        Ok(value.map(|s| s.to_string()))
    }
}
