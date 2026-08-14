//! `frust-i18n`: a Fluent Project-based internationalization/localization
//! plugin — locale-aware message resolution, system-locale detection, and
//! (optionally) ICU4X-backed number/date/plural formatting.
//!
//! # Charter: platform-plugin core, optional facade glue
//!
//! Like `frust-native-widgets` (see `docs/PLUGINS_CODE_STANDARDS.md`'s
//! Plugin Conventions), this crate ships as ONE crate rather than splitting
//! into a platform-core plus facade-glue pair: the `frust-api` feature
//! (default-on) gates the optional `frust` dependency — [`reactive`]'s
//! signal-driven locale binding. `cargo check -p frust-i18n
//! --no-default-features` must still hold the platform-plugin charter line
//! — no `frust` facade crate anywhere in the tree (`cargo tree -p
//! frust-i18n --no-default-features -e normal` is the tripwire). System
//! locale detection ([`detect`]) needs `frust-plugin` on Android (a JNI
//! read of `Resources.getSystem().getConfiguration().getLocales()`) but
//! reaches the Apple runtime through `objc2`/`objc2-foundation` directly
//! (`NSLocale`, no platform-handle init needed) and the rest of desktop
//! through `sys-locale` — see `docs/PLUGINS_ARCHITECTURE.md`'s Layer
//! Dependencies for the shared shape every plugin's target-gated FFI split
//! follows, and this crate's own `Cargo.toml` for exactly how its split
//! differs from `frust-clipboard`'s/`frust-haptics`'s.
//!
//! # Engine: fluent-bundle direct, no `fluent-templates`
//!
//! [`engine`] resolves messages through `fluent-bundle`'s
//! `FluentBundle`/`FluentResource` directly — this crate owns bundle
//! construction, locale negotiation (`fluent-langneg`), and fallback
//! resolution itself rather than delegating to `fluent-templates`'s
//! `Loader` abstraction (a deliberate scope decision, not an oversight).
//!
//! # `formatting`: independently toggleable from `frust-api`
//!
//! The `formatting` feature compiles in ICU4X locale-aware number/date/
//! plural formatting ([`fmt`]) — `icu_decimal`/`icu_datetime`/
//! `icu_plurals`/`icu_experimental`. It is a separate feature from
//! `frust-api` (both default-on, but a build can drop either
//! independently): a headless service that only needs message lookup and
//! no reactive glue keeps `formatting` while dropping `frust-api`, and vice
//! versa. All four combinations — default, `--no-default-features`,
//! `--features formatting`, `--features frust-api` — compile.
//!
//! # Module map
//!
//! [`error`] — [`I18nError`], the crate's one public error enum, every
//! variant a later task needs pre-planted (see that module's own doc).
//! [`locale`] — [`Locale`], the `unic_langid`-backed BCP-47 locale newtype.
//! [`detect`] — system-locale detection ([`system_locales`]: ordered
//! preference list, re-queried per call). [`engine`] — Fluent bundle
//! loading/negotiation/resolution ([`Engine`]/[`LocaleSet`]). [`fmt`]
//! (feature `formatting`) — ICU4X formatting (stub; a later task fills it
//! in). [`reactive`] (feature `frust-api`) — signal-driven locale binding
//! for a `frust` app (stub; a later task fills it in). The `locales!`
//! compile-time bundle loader lives in the sibling `frust-i18n-macros`
//! crate, re-exported here.

mod detect;
mod engine;
mod error;
mod locale;

#[cfg(feature = "formatting")]
pub mod fmt;
#[cfg(feature = "frust-api")]
mod reactive;

pub use detect::system_locales;
pub use engine::{Engine, FluentFunction, LocaleSet};
pub use error::I18nError;
pub use frust_i18n_macros::locales;
pub use locale::Locale;

#[cfg(feature = "frust-api")]
pub use reactive::active_locale;

/// Builds a [`fluent_bundle::FluentArgs`] from `key => value` pairs —
/// shorthand for the more verbose `FluentArgs::new()` plus repeated
/// `.set(...)` calls every message-formatting call site would otherwise
/// need.
///
/// ```
/// let args = frust_i18n::args!("name" => "Ada", "count" => 3);
/// assert_eq!(args.iter().count(), 2);
/// ```
#[macro_export]
macro_rules! args {
    ($($key:expr => $value:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut args = $crate::__private::fluent_bundle::FluentArgs::new();
        $(args.set($key, $value);)*
        args
    }};
}

// Not part of the public API — exists only so `args!`'s expansion can name
// `fluent_bundle` through this crate regardless of whether the macro's
// caller depends on that crate directly (the standard `$crate::__private`
// re-export idiom, e.g. `serde_json::json!`'s own shape).
#[doc(hidden)]
pub mod __private {
    pub use fluent_bundle;
}
