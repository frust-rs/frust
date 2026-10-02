//! [`I18nError`]: the crate's one public error enum every fallible
//! `frust-i18n` API returns — `thiserror`-derived per
//! `docs/CODE_STANDARDS.md`'s Error Handling rule.
//!
//! Every variant a later task needs is pre-planted here (the
//! overlap-elimination shape this whole crate scaffold follows — see
//! `src/lib.rs`'s crate doc): a later task may **use** this enum but must
//! not **modify** it; if a variant it needs is missing, it reports the gap
//! rather than editing this file.

/// The crate's public error type.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum I18nError {
    /// No bundle in the negotiated locale chain has a message for `key`.
    #[error("no message for key '{key}' in locale '{locale}'")]
    MissingMessage {
        /// The requested Fluent message id.
        key: String,
        /// The locale that was checked (its BCP-47 identifier string).
        locale: String,
    },

    /// A message's Fluent pattern references a variable the caller's
    /// `FluentArgs` didn't supply.
    #[error("message '{key}' references undefined variable '{var}'")]
    MissingVariable {
        /// The Fluent message id.
        key: String,
        /// The undefined variable name.
        var: String,
    },

    /// Fluent pattern resolution produced one or more format errors.
    #[error("formatting error: {0}")]
    Format(String),

    /// No locale is registered at all — the compiled bundle set is empty.
    #[error("no locales are registered")]
    NoLocales,

    /// The current build target has no supported locale-detection backend
    /// (`src/detect`'s cfg arms don't cover it).
    #[error("unsupported platform for locale detection")]
    UnsupportedPlatform,

    /// System locale detection failed for a reason the platform backend
    /// itself reports.
    #[error("locale detection failed: {0}")]
    Detection(String),

    /// The `formatting` feature is not compiled in — no `icu_decimal`/
    /// `icu_datetime`/`icu_plurals`/`icu_experimental` formatter is
    /// available.
    #[error("ICU formatting is unavailable — enable the `formatting` feature")]
    FormattingUnavailable,
}
