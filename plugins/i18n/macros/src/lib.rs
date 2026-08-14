//! `frust-i18n-macros`: the `locales!` compile-time Fluent-bundle loader for
//! `frust-i18n`.
//!
//! # Charter
//!
//! A plain proc-macro crate, split from `frust-i18n` only because
//! `proc-macro = true` crates cannot also export ordinary items — the
//! mechanical reason a `<crate>-macros` sibling exists, not a layering
//! choice. `frust-i18n` re-exports [`locales!`](macro@locales) at its own
//! crate root (`frust_i18n::locales`), so a caller never depends on this
//! crate directly.
//!
//! # `locales!` (stub — filled by a later task)
//!
//! ```ignore
//! frust_i18n::locales!("locales/");
//! ```
//!
//! Takes one string-literal argument: a path (relative to the invoking
//! crate's `CARGO_MANIFEST_DIR`) to a directory of per-locale
//! subdirectories, each holding one or more `.ftl` message files (the
//! Fluent Project syntax). The real implementation walks that directory at
//! macro-expansion time, parses every `.ftl` file with `fluent-syntax` (a
//! compile-time syntax check — a malformed message fails the build, not
//! runtime), validates each directory name as a BCP-47 identifier via
//! `unic-langid`, and expands to a `const`/`static` table `frust-i18n`'s
//! engine module (`plugins/i18n/src/engine`) loads bundles from. This stub
//! only validates the argument shape and expands to nothing.

use proc_macro::TokenStream;
use quote::quote;
use syn::{LitStr, parse_macro_input};

/// Loads a directory of per-locale Fluent bundles at compile time.
///
/// See the crate doc for the intended real shape — filled by a later task.
#[proc_macro]
pub fn locales(input: TokenStream) -> TokenStream {
    // Only the argument shape is validated for now — a real path/file walk
    // is a later task's job (see the crate doc).
    let _path: LitStr = parse_macro_input!(input as LitStr);
    quote! {}.into()
}
