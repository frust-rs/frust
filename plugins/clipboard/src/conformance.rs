//! Shared conformance suite for every [`Backend`] implementation.
//!
//! `#[cfg(test)]`-only: [`run_conformance_suite`] exercises the full
//! contract [`crate::Clipboard`] promises — set→get round trip, overwrite,
//! empty string, unicode (emoji + CJK), and a fresh backend's first read
//! never erroring — against any backend factory.
//!
//! Unlike `frust-secure-storage`'s/`frust-shared-preferences`' suites, there
//! is no store-name isolation to assert: a clipboard is one global OS slot,
//! so the factory takes no name argument and every sub-test shares the same
//! underlying clipboard, exactly like a real app would.

use crate::Backend;

/// Run the full suite against backends `factory` produces. Each call to
/// `factory` opens a fresh handle onto the *same* underlying clipboard (there
/// is only one), so sub-tests run in sequence and each sets state before
/// asserting on it.
pub(crate) fn run_conformance_suite(factory: &dyn Fn() -> Box<dyn Backend>) {
    round_trip(factory());
    overwrite(factory());
    empty_string(factory());
    unicode(factory());
    fresh_backend_read_is_ok(factory());
}

fn round_trip(backend: Box<dyn Backend>) {
    backend.set_text("abc123").unwrap();
    assert_eq!(backend.get_text().unwrap(), Some("abc123".to_string()));
}

fn overwrite(backend: Box<dyn Backend>) {
    backend.set_text("first").unwrap();
    assert_eq!(backend.get_text().unwrap(), Some("first".to_string()));

    backend.set_text("second").unwrap();
    assert_eq!(backend.get_text().unwrap(), Some("second".to_string()));
}

/// Crate-wide contract (see `crate::Clipboard::get_text`'s doc): a clipboard
/// explicitly set to the empty string reads back as `Ok(None)`, identical to
/// a clipboard that was never set — there is no way for a caller to
/// distinguish the two through this API, and every backend holds to this one
/// story rather than three different ones.
fn empty_string(backend: Box<dyn Backend>) {
    backend.set_text("").unwrap();
    assert_eq!(backend.get_text().unwrap(), None);
}

fn unicode(backend: Box<dyn Backend>) {
    let emoji = "🎉✨ hello world 🚀";
    backend.set_text(emoji).unwrap();
    assert_eq!(backend.get_text().unwrap(), Some(emoji.to_string()));

    let cjk = "日本語のテキスト、中文文本";
    backend.set_text(cjk).unwrap();
    assert_eq!(backend.get_text().unwrap(), Some(cjk.to_string()));

    let mixed = "héllo 日本語 🎉✨  spaces  ";
    backend.set_text(mixed).unwrap();
    assert_eq!(backend.get_text().unwrap(), Some(mixed.to_string()));
}

/// A fresh backend handle's first read never errors — even though the
/// clipboard is a shared OS resource that may hold arbitrary content from
/// outside this test run (unlike a fresh named store, "empty" isn't
/// assertable here), `get_text` on it must still return `Ok(_)`, never
/// propagate a backend failure just from being freshly opened.
fn fresh_backend_read_is_ok(backend: Box<dyn Backend>) {
    backend.get_text().unwrap();
}
