//! Fluent bundle loading, locale negotiation, and message resolution.
//!
//! filled by a later task: owns `fluent_bundle::FluentBundle`/
//! `FluentResource` construction from the `locales!`-generated table
//! (`frust-i18n-macros`), `fluent_langneg::negotiate_languages` resolution
//! against `detect::system_locales`, and pattern formatting/argument
//! substitution (returning [`crate::I18nError::MissingMessage`]/
//! [`crate::I18nError::MissingVariable`]/[`crate::I18nError::Format`] as
//! appropriate).

use crate::Locale;

/// Returns the locales this crate has a compiled bundle for.
///
/// filled by a later task — always empty until then.
#[allow(dead_code)] // not yet wired into a public lookup API; a later task adds one
pub fn available_locales() -> Vec<Locale> {
    Vec::new()
}
