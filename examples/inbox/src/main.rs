//! Desktop entry point for the inbox demo (spec §5.5, §12.9).
//!
//! Opens the inbox screen in the preview window: a `clean_signals`
//! `ControllerCore` drives a fake async repo (with a retried first-attempt
//! failure) into an `AsyncState` signal that the [`Inbox`](inbox::Inbox)
//! component renders — Loading first, then the messages. The shared component
//! and controller live in the crate's library root so the headless async test
//! can drive them without a window.

use inbox::InboxApp;

fn main() {
    forgekit::run(InboxApp).unwrap();
}
