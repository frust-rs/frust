//! Desktop (macOS/Linux/Windows) backend: no share sheet, so copy to the
//! clipboard through `frust-clipboard`.

use frust_clipboard::{Clipboard, ClipboardError};

use crate::{ShareError, ShareOutcome};

/// Copy `text` to the clipboard. The clipboard error is reduced to its kind
/// (never its message) so nothing payload-derived reaches `ShareError`.
pub(crate) fn share(text: &str) -> Result<ShareOutcome, ShareError> {
    Clipboard::set_text(text).map_err(|e| {
        let kind = match e {
            ClipboardError::PlatformNotInitialized => "clipboard not initialized",
            ClipboardError::NotAvailable(_) => "clipboard unavailable",
            _ => "clipboard error",
        };
        ShareError::Platform(kind.to_string())
    })?;
    Ok(ShareOutcome::CopiedToClipboard)
}
