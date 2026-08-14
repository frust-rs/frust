//! Fluent bundle loading, locale negotiation, and message resolution.
//!
//! [`Engine`] is this crate's message core: it owns one immutable
//! `FluentBundle` per registered locale (built from the [`LocaleSet`] the
//! `locales!` macro constructs), negotiates a requested locale list against
//! the registered ones ([`Engine::negotiate`]), and resolves a message
//! through the resulting fallback chain ([`Engine::resolve`]).
//!
//! ```ignore
//! let engine = Engine::new(
//!     LocaleSet::new("en".parse()?)
//!         .with_locale("en".parse()?, [("app.ftl", EN_SOURCE)])
//!         .with_locale("de".parse()?, [("app.ftl", DE_SOURCE)]),
//! )?;
//! let chain = engine.negotiate(&system_locales());
//! let greeting = engine.resolve(&chain, "hello", Some(&args!("name" => "Ada")))?;
//! ```
//!
//! # Concurrency: build once, share by `&self`
//!
//! Bundles are built in [`Engine::new`] and never mutated afterward, and
//! they are `fluent-bundle`'s **concurrent** specialization
//! (`FluentBundle::new_concurrent`, whose `IntlLangMemoizer` is `Mutex`-backed),
//! so a built bundle is `Send + Sync` and formatting takes `&self`. That
//! makes [`Engine`] shareable as-is — no per-call bundle reconstruction from
//! `Arc<FluentResource>`s, and no lock of this crate's own around
//! resolution. The bundles still hold their resources as `Arc`s so a future
//! layer needing a differently-configured bundle (say, per-call functions)
//! can rebuild one without re-parsing any FTL. A `tests` assertion pins the
//! `Send + Sync` property, since it rests on an upstream type's auto traits.
//!
//! # Layout
//!
//! [`bundles`] builds a locale's bundle (concatenation rule, function seam),
//! [`negotiate`] ranks requested against available locales, and [`resolve`]
//! walks the resulting chain.

mod bundles;
mod negotiate;
mod resolve;

use std::sync::Arc;

use fluent_bundle::{FluentArgs, FluentValue};
use unic_langid::LanguageIdentifier;

use crate::{I18nError, Locale};

pub use bundles::FluentFunction;

use bundles::LocaleBundle;

/// The registered message set an [`Engine`] is built from: the FTL sources
/// of every shipped locale plus the fallback locale, isolation, and function
/// registrations to build their bundles with.
///
/// This is the shape `locales!` expands to — each locale contributes its
/// directory's `(file name, FTL source)` pairs, where the file name is
/// carried for diagnostics only (nothing is read from disk at runtime).
pub struct LocaleSet {
    fallback: Locale,
    use_isolating: bool,
    entries: Vec<LocaleEntry>,
    functions: Vec<(&'static str, FluentFunction)>,
}

/// One locale's registered sources.
struct LocaleEntry {
    locale: Locale,
    files: Vec<(&'static str, &'static str)>,
}

impl LocaleSet {
    /// Starts an empty set that falls back to `fallback`.
    ///
    /// The fallback closes every negotiated chain, so it is the locale whose
    /// messages must be complete; register it with [`with_locale`](Self::with_locale)
    /// like any other.
    pub fn new(fallback: Locale) -> Self {
        Self {
            fallback,
            use_isolating: true,
            entries: Vec::new(),
            functions: Vec::new(),
        }
    }

    /// Registers one locale's `(file name, FTL source)` pairs.
    ///
    /// Registering the same locale twice appends to it, so a caller may add
    /// a directory's files in several calls; within a locale the earliest
    /// definition of a duplicated message id wins (see [`bundles`]).
    pub fn with_locale(
        mut self,
        locale: Locale,
        files: impl IntoIterator<Item = (&'static str, &'static str)>,
    ) -> Self {
        let files = files.into_iter();
        match self.entries.iter_mut().find(|entry| entry.locale == locale) {
            Some(entry) => entry.files.extend(files),
            None => self.entries.push(LocaleEntry {
                locale,
                files: files.collect(),
            }),
        }
        self
    }

    /// Switches Unicode bidi isolation on (the default) or off.
    ///
    /// With isolation on, every interpolated placeable is wrapped in
    /// FSI/PDI (`U+2068`/`U+2069`) so a right-to-left argument cannot
    /// reorder the text around it. Turn it off only where the consumer
    /// cannot handle the marks (plain-text output, exact-match assertions).
    pub fn with_isolating(mut self, use_isolating: bool) -> Self {
        self.use_isolating = use_isolating;
        self
    }

    /// Registers a Fluent function (`{ NUMBER($n) }`) into every locale's
    /// bundle.
    ///
    /// The registration seam a formatting layer plugs `NUMBER`/`DATETIME`
    /// into; locale-aware behavior comes from the bundle the function runs
    /// in, not from the function's own captures.
    pub fn with_function(
        mut self,
        name: &'static str,
        function: impl for<'a> Fn(&[FluentValue<'a>], &FluentArgs<'_>) -> FluentValue<'a>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.functions.push((name, Arc::new(function)));
        self
    }

    /// The locale that closes every negotiated chain.
    pub fn fallback(&self) -> &Locale {
        &self.fallback
    }
}

// Hand-written: a registered function is a `dyn Fn` with no `Debug` of its
// own, so the set reports the shape a reader can act on (which locales, how
// many sources each, which function ids) instead of deriving nothing.
impl std::fmt::Debug for LocaleSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocaleSet")
            .field("fallback", &self.fallback.to_string())
            .field("use_isolating", &self.use_isolating)
            .field(
                "locales",
                &self
                    .entries
                    .iter()
                    .map(|entry| (entry.locale.to_string(), entry.files.len()))
                    .collect::<Vec<_>>(),
            )
            .field(
                "functions",
                &self
                    .functions
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// The message core: parsed bundles, locale negotiation, and fallback-chain
/// resolution.
///
/// Immutable once built and `Send + Sync` — see this module's *Concurrency*
/// note.
pub struct Engine {
    bundles: Vec<LocaleBundle>,
    available: Vec<LanguageIdentifier>,
    fallback: Locale,
    fallback_id: LanguageIdentifier,
}

impl Engine {
    /// Parses every registered source into its locale's bundle.
    ///
    /// # Errors
    ///
    /// [`I18nError::NoLocales`] when `set` registers no locale at all;
    /// [`I18nError::Format`] when a source fails to parse or a function id
    /// collides. A fallback locale with no registered messages is not an
    /// error — it is logged, and negotiation still appends it — since the
    /// chain simply skips a locale that owns nothing.
    pub fn new(set: LocaleSet) -> Result<Self, I18nError> {
        if set.entries.is_empty() {
            return Err(I18nError::NoLocales);
        }

        let mut bundles = Vec::with_capacity(set.entries.len());
        let mut available = Vec::with_capacity(set.entries.len());
        for entry in &set.entries {
            let bundle = bundles::build(
                &entry.locale,
                &entry.files,
                set.use_isolating,
                &set.functions,
            )?;
            available.push(lang_id(&entry.locale));
            bundles.push(LocaleBundle {
                locale: entry.locale.clone(),
                bundle,
            });
        }

        if !bundles.iter().any(|entry| entry.locale == set.fallback) {
            log::warn!(
                "frust-i18n: fallback locale `{}` has no registered messages",
                set.fallback
            );
        }

        Ok(Self {
            fallback_id: lang_id(&set.fallback),
            fallback: set.fallback,
            bundles,
            available,
        })
    }

    /// The locales this engine has a bundle for, in registration order.
    pub fn available_locales(&self) -> Vec<Locale> {
        self.bundles
            .iter()
            .map(|entry| entry.locale.clone())
            .collect()
    }

    /// The locale that closes every negotiated chain.
    pub fn fallback_locale(&self) -> &Locale {
        &self.fallback
    }

    /// Resolves `requested` (most-preferred first, e.g. the platform's
    /// locale list) into the fallback chain [`resolve`](Self::resolve) walks.
    ///
    /// Never empty: the declared fallback closes it.
    pub fn negotiate(&self, requested: &[Locale]) -> Vec<Locale> {
        negotiate::negotiate(requested, &self.available, &self.fallback_id)
    }

    /// Formats `key` from the first locale in `chain` that has it.
    ///
    /// `key` is a message id, optionally suffixed `.attribute`.
    ///
    /// # Errors
    ///
    /// [`I18nError::MissingMessage`] when no locale in the chain has the
    /// message (the error names the key and every locale tried);
    /// [`I18nError::MissingVariable`] when the message references a
    /// `$variable` `args` didn't supply; [`I18nError::Format`] for any other
    /// resolution failure.
    pub fn resolve(
        &self,
        chain: &[Locale],
        key: &str,
        args: Option<&FluentArgs<'_>>,
    ) -> Result<String, I18nError> {
        resolve::resolve(&self.bundles, chain, key, args)
    }
}

// Hand-written for the same reason as `LocaleSet`'s: a built bundle holds
// boxed functions and a memoizer, neither of which is `Debug`.
impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("fallback", &self.fallback.to_string())
            .field(
                "locales",
                &self
                    .bundles
                    .iter()
                    .map(|entry| entry.locale.to_string())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Recovers the `unic_langid` identifier behind a [`Locale`].
///
/// [`Locale`] exposes only its `language`/`script`/`region` subtags, so the
/// canonical BCP-47 string it `Display`s is the lossless route back to the
/// identifier `fluent-langneg` and `FluentBundle` want; a re-parse of that
/// canonical form cannot fail, and the arm that would only run if it did
/// rebuilds from subtags (dropping variants) rather than panicking.
fn lang_id(locale: &Locale) -> LanguageIdentifier {
    locale.as_lang_id().clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    const EN: &str = "\
hello = Hello, { $name }!
only-en = English only";
    const DE: &str = "hello = Hallo, { $name }!";

    fn locale(tag: &str) -> Locale {
        tag.parse().expect("valid locale")
    }

    fn engine() -> Engine {
        Engine::new(
            LocaleSet::new(locale("en"))
                .with_locale(locale("en"), [("app.ftl", EN)])
                .with_locale(locale("de"), [("app.ftl", DE)])
                .with_isolating(false),
        )
        .expect("engine builds")
    }

    #[test]
    fn engine_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Engine>();
    }

    #[test]
    fn an_empty_set_has_no_locales() {
        let error = Engine::new(LocaleSet::new(locale("en"))).expect_err("nothing registered");

        assert!(matches!(error, I18nError::NoLocales), "{error:?}");
    }

    #[test]
    fn a_malformed_source_fails_construction() {
        let error = Engine::new(
            LocaleSet::new(locale("en")).with_locale(locale("en"), [("bad.ftl", "= no id")]),
        )
        .expect_err("malformed FTL is rejected");

        assert!(matches!(error, I18nError::Format(_)), "{error:?}");
    }

    #[test]
    fn registered_locales_are_reported_in_order() {
        let engine = engine();

        let available: Vec<String> = engine
            .available_locales()
            .iter()
            .map(|locale| locale.to_string())
            .collect();

        assert_eq!(available, ["en", "de"]);
        assert_eq!(engine.fallback_locale().to_string(), "en");
    }

    #[test]
    fn negotiation_feeds_resolution_through_the_fallback() {
        let engine = engine();

        let chain = engine.negotiate(&[locale("de-AT")]);

        assert_eq!(
            chain.iter().map(|l| l.to_string()).collect::<Vec<_>>(),
            ["de", "en"]
        );
        assert_eq!(
            engine
                .resolve(&chain, "hello", Some(&args!("name" => "Ada")))
                .expect("resolves in de"),
            "Hallo, Ada!"
        );
        assert_eq!(
            engine
                .resolve(&chain, "only-en", None)
                .expect("resolves in the fallback"),
            "English only"
        );
    }

    #[test]
    fn a_missing_key_names_the_key_and_the_chain() {
        let engine = engine();
        let chain = engine.negotiate(&[locale("de")]);

        let error = engine
            .resolve(&chain, "nope", None)
            .expect_err("no such message");

        match error {
            I18nError::MissingMessage { key, locale } => {
                assert_eq!(key, "nope");
                assert_eq!(locale, "de, en");
            }
            other => panic!("expected MissingMessage, got {other:?}"),
        }
    }

    #[test]
    fn several_registrations_of_one_locale_concatenate() {
        let engine = Engine::new(
            LocaleSet::new(locale("en"))
                .with_locale(locale("en"), [("a.ftl", "a = A")])
                .with_locale(locale("en"), [("b.ftl", "b = B")]),
        )
        .expect("engine builds");
        let chain = engine.negotiate(&[locale("en")]);

        assert_eq!(engine.available_locales().len(), 1);
        assert_eq!(engine.resolve(&chain, "a", None).expect("a"), "A");
        assert_eq!(engine.resolve(&chain, "b", None).expect("b"), "B");
    }

    #[test]
    fn isolation_is_on_unless_switched_off() {
        let engine =
            Engine::new(LocaleSet::new(locale("en")).with_locale(locale("en"), [("app.ftl", EN)]))
                .expect("engine builds");
        let chain = engine.negotiate(&[locale("en")]);

        assert_eq!(
            engine
                .resolve(&chain, "hello", Some(&args!("name" => "Ada")))
                .expect("resolves"),
            "Hello, \u{2068}Ada\u{2069}!"
        );
    }

    #[test]
    fn a_registered_function_is_available_to_every_locale() {
        let engine = Engine::new(
            LocaleSet::new(locale("en"))
                .with_locale(locale("en"), [("app.ftl", r#"n = { COUNT("x") }"#)])
                .with_locale(locale("de"), [("app.ftl", r#"n = { COUNT("xx") }"#)])
                .with_isolating(false)
                .with_function("COUNT", |positional, _named| match positional {
                    [FluentValue::String(text)] => FluentValue::from(text.len()),
                    _ => FluentValue::Error,
                }),
        )
        .expect("engine builds");

        assert_eq!(
            engine
                .resolve(&[locale("en")], "n", None)
                .expect("resolves in en"),
            "1"
        );
        assert_eq!(
            engine
                .resolve(&[locale("de")], "n", None)
                .expect("resolves in de"),
            "2"
        );
    }

    #[test]
    fn an_unregistered_fallback_still_closes_the_chain() {
        let engine =
            Engine::new(LocaleSet::new(locale("fr")).with_locale(locale("en"), [("app.ftl", EN)]))
                .expect("engine builds");

        let chain = engine.negotiate(&[locale("de")]);

        assert_eq!(
            chain.iter().map(|l| l.to_string()).collect::<Vec<_>>(),
            ["fr"]
        );
        assert!(matches!(
            engine.resolve(&chain, "only-en", None),
            Err(I18nError::MissingMessage { .. })
        ));
    }
}
