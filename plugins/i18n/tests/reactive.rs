//! Headless integration tests for `frust-i18n`'s reactive context surface,
//! driven from outside the crate — the shape
//! `plugins/clean-signals-frust/tests/glue.rs` uses for its own hook tests.
//!
//! **Scope note.** This file was authored while `src/reactive.rs`'s public
//! surface (`I18n`, `provide_i18n`, `use_i18n`, `expect_i18n`) was not yet
//! re-exported from the crate root (that wiring has since landed in
//! `lib.rs`), so the richer construct/resolve/
//! `set_locale`/re-resolve, negotiation-at-construction, missing-key-softens,
//! and signal-wakes-a-tracked-scope scenarios are covered as in-crate
//! `#[cfg(test)] mod tests` inside `src/reactive.rs` instead — this crate's
//! own established per-module test convention (`src/engine/mod.rs`,
//! `src/engine/negotiate.rs`, `src/engine/resolve.rs`, `src/locale.rs`,
//! `src/detect/mod.rs` all test this way, not via `tests/*.rs`).
//!
//! This file covers what *is* externally reachable today: [`active_locale`]'s
//! no-provider contract, exercised the way an app crate would actually call
//! it (through the crate's public root, with and without an ambient reactive
//! `Owner`).
//!
//! Gated on the `frust-api` feature (default-on) — `active_locale` doesn't
//! exist without it, so `cargo test -p frust-i18n --no-default-features`
//! must skip this file entirely rather than fail to compile.
#![cfg(feature = "frust-api")]

use frust_i18n::active_locale;
use frust_reactive::Owner;

#[test]
fn active_locale_is_none_with_no_provider() {
    let owner = Owner::new();
    owner.set();

    assert_eq!(active_locale(), None);
}

#[test]
fn active_locale_is_none_with_no_ambient_owner_at_all() {
    // No `Owner::new().set()` here: `use_context` (which `active_locale`
    // delegates to via `use_i18n`) must not panic outside any owner scope —
    // it must simply report nothing provided.
    assert_eq!(active_locale(), None);
}
