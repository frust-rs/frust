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
//! `UIPasteboard` carries **no documented main-thread requirement**: unlike
//! most UIKit types it is not `@MainActor`-isolated, and its Swift overlay
//! declares no `Sendable`/main-actor annotation restricting which thread may
//! call it — there is simply no documented hazard to work around, not a
//! documented *guarantee* of any-thread safety (no such guarantee is
//! written down anywhere in Apple's reference for this class). `objc2-ui-kit`'s
//! generated bindings mark `string`/`setString:` `unsafe` only because the
//! header's "This might not be thread-safe" note is conservative
//! boilerplate (header-translator's default for any non-atomic
//! Objective-C property), not a documented hazard either. Given the
//! absence of either a documented requirement or a documented hazard, this
//! backend makes the deliberate choice to call these methods directly from
//! the caller's thread (matching every other backend in this crate, all of
//! which are synchronous) rather than hopping to the main thread on
//! spec — each call `# Safety`-noted below per
//! `docs/CODE_STANDARDS.md`'s sanctioned-unsafe convention.
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
//! # Sensitivity marking (`set_text_sensitive`)
//!
//! [`Backend::set_text_sensitive`] writes through `setItems(_:options:)`
//! (rather than the plain `setString:` [`Backend::set_text`] uses) with
//! `UIPasteboardOptionLocalOnly: true` in the options dictionary and a
//! single `{"public.utf8-plain-text": <text>}` item — `localOnly` blocks
//! Universal Clipboard/Handoff from replicating the item to the user's
//! other signed-in devices. No `UIPasteboardOptionExpirationDate` is set: an
//! auto-expiring clipboard is a plausible future opt-in, but an opinionated
//! default this crate does not want to impose today.
//!
//! # `unsafe`
//!
//! Confined to this module, mirroring `frust-shared-preferences`'s `apple`
//! backend precedent: `UIPasteboard::string`/`setString:`/`setItems_options`
//! are the calls objc2-ui-kit marks `unsafe` (plus reading the
//! `UIPasteboardOptionLocalOnly` `extern` static, itself always `unsafe` to
//! reference), each `# Safety`-noted. This backend holds no Objective-C
//! reference across calls (it re-fetches `generalPasteboard` per operation,
//! a cheap singleton lookup), so [`AppleClipboard`] is a plain value,
//! trivially `Send + Sync`.

use objc2::runtime::AnyObject;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};
use objc2_ui_kit::{UIPasteboard, UIPasteboardOption, UIPasteboardOptionLocalOnly};

use crate::{Backend, ClipboardError};

/// The Uniform Type Identifier this crate writes plain-text pasteboard items
/// under (`kUTTypeUTF8PlainText`'s string value) — the same UTI
/// `UIPasteboard.setString:`'s own plain-text item carries internally, so a
/// [`Backend::set_text_sensitive`] write reads back identically to a
/// [`Backend::set_text`] one via [`Backend::get_text`]/`UIPasteboard.string`.
const UTI_PLAIN_TEXT: &str = "public.utf8-plain-text";

/// The iOS backend. Stateless — every operation re-fetches
/// `UIPasteboard.generalPasteboard`, a cheap singleton lookup.
pub(crate) struct AppleClipboard;

impl Backend for AppleClipboard {
    fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        let pasteboard = UIPasteboard::generalPasteboard();
        let value = NSString::from_str(text);
        // SAFETY: `setString:` is a plain property write with a valid
        // `NSString` argument; no documented thread hazard exists for
        // `UIPasteboard` (see the module doc's *Thread-safety*).
        unsafe { pasteboard.setString(Some(&value)) };
        Ok(())
    }

    fn get_text(&self) -> Result<Option<String>, ClipboardError> {
        let pasteboard = UIPasteboard::generalPasteboard();
        // SAFETY: `string` is a plain property read; no documented thread
        // hazard exists for `UIPasteboard` (see the module doc's
        // *Thread-safety*). Reading here may show the iOS 14+ paste banner
        // (module doc) — expected, not an error.
        let value = unsafe { pasteboard.string() };
        let text = value.map(|s| s.to_string());
        // Crate-wide contract (see `crate::Clipboard::get_text`'s doc): an
        // empty string reads back as `Ok(None)`, the same as a genuinely
        // absent pasteboard string.
        Ok(text.filter(|s| !s.is_empty()))
    }

    fn set_text_sensitive(&self, text: &str) -> Result<(), ClipboardError> {
        let pasteboard = UIPasteboard::generalPasteboard();
        let type_key = NSString::from_str(UTI_PLAIN_TEXT);
        let value = NSString::from_str(text);
        let value_obj: &AnyObject = &value;
        let item = NSDictionary::from_slices(&[&*type_key], &[value_obj]);
        let item_ref: &NSDictionary<NSString, AnyObject> = &item;
        let items = NSArray::from_slice(&[item_ref]);
        // SAFETY: reading an `extern` static reference (edition-2024
        // unsafe-extern rule) — linker-provided and non-null wherever UIKit
        // is linked, matching `plugins/camera/src/apple.rs`'s precedent for
        // the same pattern with a CoreVideo constant.
        let local_only_key: &UIPasteboardOption = unsafe { UIPasteboardOptionLocalOnly };
        let local_only_value = NSNumber::new_bool(true);
        let local_only_obj: &AnyObject = &local_only_value;
        let options = NSDictionary::from_slices(&[local_only_key], &[local_only_obj]);
        // SAFETY: `setItems:options:` requires the `items` array's
        // dictionaries map UTI keys to representation objects of a type
        // `UIPasteboard` accepts (`NSString` is one) and `options`' keys be
        // recognized `UIPasteboardOption` constants with correctly-typed
        // values (`UIPasteboardOptionLocalOnly` takes an `NSNumber` bool) —
        // both hold here. No documented thread hazard exists for
        // `UIPasteboard` (see the module doc's *Thread-safety*).
        unsafe { pasteboard.setItems_options(&items, &options) };
        Ok(())
    }
}
