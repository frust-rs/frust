//! ICU4X-backed locale-aware formatting (numbers, dates, plurals).
//!
//! filled by a later task: `icu_decimal`/`icu_datetime`/`icu_plurals`
//! formatters keyed by [`crate::Locale`], plus `icu_experimental` for
//! locale-aware list/plural-range formatting layered underneath Fluent's
//! own argument-substitution hooks (`FLUENT_FORMATTED_VALUE`-style). Only
//! compiled with the `formatting` feature (see `src/lib.rs`'s crate doc) —
//! independently toggleable from `frust-api`, so a headless build can
//! format numbers/dates/plurals with no facade dependency at all.
//!
//! `pub mod` (unlike `detect`/`engine`, both crate-private): this module's
//! contents are meant to be part of `frust-i18n`'s public API once the real
//! formatters land (`frust_i18n::fmt::...`), not routed through the
//! `engine`'s message-lookup surface.

use crate::Locale;

/// Returns the locales this crate ships compiled ICU4X formatting data for.
///
/// filled by a later task — always empty until then.
pub fn supported_locales() -> Vec<Locale> {
    Vec::new()
}
