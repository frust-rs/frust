//! Signal-driven locale binding for a `frust` app.
//!
//! [`I18n`] pairs an [`Engine`] with a tracked `RwSignal<Locale>` holding the
//! app's active (negotiated) locale: [`I18n::locale`], [`I18n::t`]/
//! [`I18n::t_args`], and a `locales!`-generated typed-key function (through
//! the [`Resolve`] impl below — the shared seam `src/engine/mod.rs` documents)
//! all read that signal first, so a rebuild that calls through any of them
//! subscribes to locale switches exactly like it subscribes to any other app
//! signal. [`I18n::set_locale`] re-negotiates and writes the signal, which
//! wakes the shell's next frame through the normal tracked-write →
//! `FrameWaker` path (`docs/CORE_ARCHITECTURE.md`'s rebuild-wake flow) — this
//! module never calls a waker directly.
//!
//! [`provide_i18n`]/[`use_i18n`]/[`expect_i18n`] mirror
//! `clean-signals-frust`'s `provide_controller`/`expect_controller` context
//! hooks (`docs/PLUGINS_ARCHITECTURE.md`'s Key Types): the signal inside
//! [`I18n`] is what makes a read reactive; context is only how the handle
//! itself reaches a component that didn't build it.
//!
//! Only compiled with the `frust-api` feature — the facade-glue half of this
//! crate's charter (see `src/lib.rs`'s crate doc's *Charter* section).
//! Production code here imports only `frust::{...}`, never `reactive_graph`
//! directly, per `docs/PLUGINS_CODE_STANDARDS.md`'s Plugin Conventions.

use std::sync::{Arc, Mutex};

use fluent_bundle::FluentArgs;
use frust::{Get, RwSignal, Set, provide_context, use_context};

use crate::{Engine, I18nError, Locale, LocaleSet, Resolve};

/// A reactive locale handle: an [`Engine`] paired with the app's active
/// locale as a tracked signal.
///
/// Cheap to [`Clone`] — every clone shares the same engine, signal, and
/// negotiated chain through one `Arc` (mirroring `clean-signals-frust`'s
/// `Arc`-shared controller handles).
#[derive(Clone)]
pub struct I18n {
    inner: Arc<Inner>,
}

struct Inner {
    engine: Engine,
    /// The active (negotiated best-match) locale — the tracked signal every
    /// [`I18n::locale`]/[`I18n::t`]/[`Resolve::resolve_message`] call reads
    /// first, so a rebuild calling through any of them subscribes to
    /// [`I18n::set_locale`].
    locale: RwSignal<Locale>,
    /// The full negotiated fallback chain behind `locale` (`locale` is
    /// always `chain[0]`, or the engine's fallback locale if negotiation
    /// somehow returned nothing — see [`negotiate_active`]). Refreshed under
    /// the same call that writes `locale`; read untracked, since only the
    /// signal read above needs to subscribe a rebuild.
    chain: Mutex<Vec<Locale>>,
    /// The raw locale list [`I18n::new`]/[`I18n::from_engine`] were given, or
    /// (after a [`I18n::set_locale`] call) the single locale most recently
    /// passed to it — **replaced**, not accumulated, same refresh discipline
    /// as `chain`. `negotiate_active` collapses a regional request like
    /// `en-GB` down to whatever bundle actually shipped (`en`); this is the
    /// only place that untouched request tag survives, and it's what
    /// [`I18n::format_locale`] recovers it from. Refreshed under the same
    /// call that writes `locale`; read untracked.
    requested: Mutex<Vec<Locale>>,
}

impl I18n {
    /// Builds an [`Engine`] from `set` and negotiates `requested` against it.
    ///
    /// The typical call site passes [`crate::system_locales`]'s result:
    ///
    /// ```ignore
    /// let i18n = I18n::new(build_locale_set(), &system_locales()?)?;
    /// ```
    ///
    /// # Errors
    ///
    /// The same contract as [`Engine::new`].
    ///
    /// # Ambient-owner contract
    ///
    /// Like `clean-signals-frust`'s `use_controller`/`provide_controller`,
    /// call this where a reactive `Owner` is ambient (app setup, or a root
    /// `Component::init`) — the signal it creates is registered against
    /// whichever owner is current so it can be disposed with it.
    pub fn new(set: LocaleSet, requested: &[Locale]) -> Result<Self, I18nError> {
        Ok(Self::from_engine(Engine::new(set)?, requested))
    }

    /// Same as [`I18n::new`] but from an already-built [`Engine`] — for a
    /// caller sharing one compiled engine across several handles (e.g.
    /// re-negotiating the same bundle set without re-parsing any FTL).
    ///
    /// Same ambient-owner contract as [`I18n::new`].
    pub fn from_engine(engine: Engine, requested: &[Locale]) -> Self {
        let (active, chain) = negotiate_active(&engine, requested);
        Self {
            inner: Arc::new(Inner {
                engine,
                locale: RwSignal::new(active),
                chain: Mutex::new(chain),
                requested: Mutex::new(requested.to_vec()),
            }),
        }
    }

    /// The active (negotiated) **message** locale.
    ///
    /// This is a *bundle* locale, not a *formatting* locale — it can only
    /// ever be one of the `.ftl` catalogs this app actually shipped (`en`,
    /// `de`, ...), because negotiation collapses a regional request like
    /// `en-GB` down to whatever bundle is available (`en`), discarding the
    /// requested region. That's the right locale for [`I18n::t`]/
    /// [`I18n::t_args`], but the **wrong** one for `frust_i18n::fmt::` calls
    /// — ICU4X ships full CLDR regional data regardless of which message
    /// bundles this app compiled in, so a UK user of an `en`-only app should
    /// still see UK-formatted numbers/dates. Use [`I18n::format_locale`] for
    /// that; see its doc for the full distinction.
    ///
    /// A tracked read: calling this inside a live rebuild subscribes it to
    /// [`I18n::set_locale`] — the same `.get()` contract every other `frust`
    /// signal read follows (`docs/CODE_STANDARDS.md`'s State & Reactivity
    /// Conventions).
    pub fn locale(&self) -> Locale {
        self.inner.locale.get()
    }

    /// The best locale to format numbers/dates/currency with — deliberately
    /// **not** the same value as [`I18n::locale`].
    ///
    /// [`I18n::locale`] is a *message* locale, constrained to whatever
    /// `.ftl` bundle negotiation actually landed on. Regional formatting has
    /// no such constraint, so `format_locale` recovers the caller's raw
    /// requested tag instead of the negotiated message locale:
    ///
    /// 1. The best requested tag whose [`Locale::language`] matches the
    ///    active message locale's language — e.g. requesting `en-GB` over an
    ///    `[en]` bundle set negotiates a message locale of `en`, but
    ///    `format_locale` still returns `en-GB`.
    /// 2. If no requested tag shares the message locale's language (the
    ///    caller asked for a locale with zero message coverage), the first
    ///    requested tag — still worth honoring for formatting; silently
    ///    substituting the message locale here would overrule a real user
    ///    preference with no upside.
    /// 3. If nothing was requested at all (an empty list passed to
    ///    [`I18n::new`]/[`I18n::from_engine`]), the active message locale —
    ///    there is no other tag to recover.
    ///
    /// Pass this, not [`I18n::locale`], to every `frust_i18n::fmt::` call.
    ///
    /// A tracked read, same contract as [`I18n::locale`]: calling this
    /// inside a live rebuild subscribes to [`I18n::set_locale`].
    pub fn format_locale(&self) -> Locale {
        // Track first: this is the subscription a later `set_locale` wakes —
        // same discipline as `resolve_message`.
        let active = self.locale();
        let requested = self
            .inner
            .requested
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match requested
            .iter()
            .find(|tag| tag.language() == active.language())
        {
            Some(tag) => tag.clone(),
            None => requested.first().cloned().unwrap_or(active),
        }
    }

    /// Re-negotiates against `requested` and writes the new active locale.
    ///
    /// Replaces (never accumulates) the requested-locale list
    /// [`I18n::format_locale`] reads — same refresh discipline as the
    /// negotiated `chain`. The write goes through the normal `RwSignal::set`
    /// → tracked-scope → `FrameWaker` path — no direct wake call here (see
    /// the module doc).
    pub fn set_locale(&self, requested: Locale) {
        let (active, chain) =
            negotiate_active(&self.inner.engine, std::slice::from_ref(&requested));
        *self.inner.chain.lock().unwrap_or_else(|e| e.into_inner()) = chain;
        *self
            .inner
            .requested
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = vec![requested];
        self.inner.locale.set(active);
    }

    /// Formats `key` with no arguments — shorthand for
    /// [`I18n::t_args`]`(key, None)`.
    pub fn t(&self, key: &str) -> String {
        self.t_args(key, None)
    }

    /// Formats `key` with `args`, softened for a build path that must never
    /// fail on a missing translation: a miss returns `key` itself and logs a
    /// `log::warn!` rather than propagating [`I18nError`].
    ///
    /// Delegates to [`Resolve::resolve_message`], which reads the locale
    /// signal first — so a caller inside a tracked rebuild still subscribes
    /// on a miss.
    pub fn t_args(&self, key: &str, args: Option<&FluentArgs<'_>>) -> String {
        match self.resolve_message(key, args) {
            Ok(text) => text,
            Err(err) => {
                log::warn!("frust-i18n: missing translation for '{key}': {err}");
                key.to_string()
            }
        }
    }
}

impl Resolve for I18n {
    /// The seam `locales!`-generated typed-key functions call through, and
    /// what [`I18n::t`]/[`I18n::t_args`] delegate to: reads the locale signal
    /// first (subscribing the caller) before resolving via the engine's
    /// untracked chain snapshot.
    fn resolve_message(
        &self,
        key: &str,
        args: Option<&FluentArgs<'_>>,
    ) -> Result<String, I18nError> {
        // Track first: this is the subscription a later `set_locale` wakes.
        let _ = self.locale();
        let chain = self.inner.chain.lock().unwrap_or_else(|e| e.into_inner());
        self.inner.engine.resolve(&chain, key, args)
    }
}

/// Negotiates `requested` against `engine`, returning the active
/// (best-match) locale alongside the full chain behind it.
///
/// [`Engine::negotiate`] never returns an empty chain (the fallback closes
/// it), so `active` always exists; the `unwrap_or_else` arm is defensive,
/// not a path exercised in practice.
fn negotiate_active(engine: &Engine, requested: &[Locale]) -> (Locale, Vec<Locale>) {
    let chain = engine.negotiate(requested);
    let active = chain
        .first()
        .cloned()
        .unwrap_or_else(|| engine.fallback_locale().clone());
    (active, chain)
}

/// Provides an app-scoped [`I18n`] handle through Frust's reactive context.
///
/// Mirrors `clean-signals-frust`'s `provide_controller`: call from app setup
/// or a root `Component::init` so every descendant can [`use_i18n`]/
/// [`expect_i18n`] it. `I18n` is cheap to clone (see its own doc), so a
/// consumer gets its own handle rather than a shared reference.
pub fn provide_i18n(i18n: I18n) {
    provide_context(i18n);
}

/// Retrieves the [`I18n`] handle previously supplied by [`provide_i18n`] in
/// an ancestor, or `None` if nothing has been provided (or there is no
/// ambient reactive owner at all).
///
/// The signal inside the returned handle is what provides reactivity; this
/// lookup itself is a plain context read and does not subscribe anything —
/// see `docs/CORE_ARCHITECTURE.md`'s "Context is not reactive" note.
pub fn use_i18n() -> Option<I18n> {
    use_context::<I18n>()
}

/// Like [`use_i18n`], but panics if nothing has been provided.
///
/// # Panics
///
/// Panics with `"expect_i18n: no I18n handle is provided in context; call
/// provide_i18n in an ancestor component first"` — the same contract as
/// `clean-signals-frust`'s `expect_controller`.
pub fn expect_i18n() -> I18n {
    use_i18n().expect(
        "expect_i18n: no I18n handle is provided in context; call provide_i18n in an ancestor \
         component first",
    )
}

/// The app's currently active locale, read from the [`I18n`] handle in
/// context if one has been [`provide_i18n`]d — `None` otherwise (no provider
/// mounted yet, or called outside any context scope).
///
/// A tracked read: calling this inside a live rebuild subscribes to locale
/// switches exactly like [`I18n::locale`] does, since it delegates there.
pub fn active_locale() -> Option<Locale> {
    use_i18n().map(|i18n| i18n.locale())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use frust_reactive::{FrameWaker, Owner, ReactiveRuntime, TrackedScope};

    use super::*;
    use crate::args;

    const EN: &str = "\
hello = Hello, { $name }!
only-en = English only";
    const DE: &str = "hello = Hallo, { $name }!";

    fn locale(tag: &str) -> Locale {
        tag.parse().expect("valid locale")
    }

    fn build_set() -> LocaleSet {
        LocaleSet::new(locale("en"))
            .with_locale(locale("en"), [("app.ftl", EN)])
            .with_locale(locale("de"), [("app.ftl", DE)])
            .with_isolating(false)
    }

    /// Installs an ambient reactive `Owner` — needed for `RwSignal::new`'s
    /// disposal registration and for `provide_context`/`use_context` to have
    /// somewhere to store/find a value. Mirrors `clean-signals-frust`'s own
    /// `tests/support::setup` shape, minus the full `ReactiveRuntime`/
    /// `RenderRoot` machinery this signal-level test doesn't need.
    fn ambient_owner() -> Owner {
        let owner = Owner::new();
        owner.set();
        owner
    }

    #[test]
    fn resolves_and_re_resolves_after_set_locale() {
        let _owner = ambient_owner();
        let i18n = I18n::new(build_set(), &[locale("de")]).expect("engine builds");

        assert_eq!(i18n.locale().to_string(), "de");
        assert_eq!(
            i18n.t_args("hello", Some(&args!("name" => "Ada"))),
            "Hallo, Ada!"
        );

        i18n.set_locale(locale("en"));

        assert_eq!(i18n.locale().to_string(), "en");
        assert_eq!(
            i18n.t_args("hello", Some(&args!("name" => "Ada"))),
            "Hello, Ada!"
        );
    }

    #[test]
    fn negotiation_at_construction_prefers_the_closest_available_match() {
        let _owner = ambient_owner();
        let i18n =
            I18n::new(build_set(), &[locale("de-CH"), locale("en-US")]).expect("engine builds");

        assert_eq!(i18n.locale().to_string(), "de");
    }

    #[test]
    fn a_missing_key_returns_the_key_itself_never_panics() {
        let _owner = ambient_owner();
        let i18n = I18n::new(build_set(), &[locale("en")]).expect("engine builds");

        assert_eq!(i18n.t("does-not-exist"), "does-not-exist");
    }

    #[test]
    fn t_falls_back_through_the_chain_like_resolve_does() {
        let _owner = ambient_owner();
        let i18n = I18n::new(build_set(), &[locale("de")]).expect("engine builds");

        // "only-en" only exists in the `en` bundle; `de`'s negotiated chain
        // still includes `en` as the declared fallback.
        assert_eq!(i18n.t("only-en"), "English only");
    }

    #[test]
    fn i18n_is_cheap_to_clone_and_shares_state() {
        let _owner = ambient_owner();
        let i18n = I18n::new(build_set(), &[locale("de")]).expect("engine builds");
        let clone = i18n.clone();

        i18n.set_locale(locale("en"));

        assert_eq!(
            clone.locale().to_string(),
            "en",
            "clones share the same underlying signal"
        );
    }

    #[test]
    fn provide_and_use_round_trip_through_context() {
        let _owner = ambient_owner();
        assert!(use_i18n().is_none(), "nothing provided yet");

        let i18n = I18n::new(build_set(), &[locale("de")]).expect("engine builds");
        provide_i18n(i18n.clone());

        let fetched = use_i18n().expect("provided handle is retrievable");
        assert_eq!(fetched.locale().to_string(), "de");
        assert_eq!(expect_i18n().locale().to_string(), "de");
        assert_eq!(active_locale(), Some(locale("de")));
    }

    #[test]
    #[should_panic(expected = "no I18n handle is provided in context")]
    fn expect_i18n_panics_when_nothing_provided() {
        let _owner = ambient_owner();
        let _ = expect_i18n();
    }

    #[test]
    fn active_locale_is_none_absent_a_provider() {
        let _owner = ambient_owner();
        assert_eq!(active_locale(), None);
    }

    // `format_locale` matrix — pins the message-vs-format locale distinction
    // `I18n::locale`/`I18n::format_locale`'s doc comments describe.

    #[test]
    fn format_locale_recovers_the_requested_region_over_a_language_only_bundle() {
        let _owner = ambient_owner();
        let i18n = I18n::new(build_set(), &[locale("en-GB")]).expect("engine builds");

        assert_eq!(
            i18n.locale().to_string(),
            "en",
            "the bundle set only ships `en`, never a region"
        );
        assert_eq!(i18n.format_locale().to_string(), "en-GB");
    }

    #[test]
    fn format_locale_matches_the_requested_tag_sharing_the_message_locales_language() {
        let _owner = ambient_owner();
        let i18n =
            I18n::new(build_set(), &[locale("de-CH"), locale("en-US")]).expect("engine builds");

        assert_eq!(i18n.locale().to_string(), "de");
        assert_eq!(i18n.format_locale().to_string(), "de-CH");
    }

    #[test]
    fn format_locale_falls_back_to_the_first_requested_tag_with_no_language_match() {
        let _owner = ambient_owner();
        let i18n = I18n::new(build_set(), &[locale("de")]).expect("engine builds");

        // `fr` has no bundle at all, so the message locale negotiates down to
        // the declared fallback (`en`) — `fr` shares no language with `en`,
        // so `format_locale` still honors the caller's own request instead of
        // silently substituting the message locale.
        i18n.set_locale(locale("fr"));

        assert_eq!(i18n.locale().to_string(), "en");
        assert_eq!(i18n.format_locale().to_string(), "fr");
    }

    #[test]
    fn format_locale_equals_locale_with_nothing_requested() {
        let _owner = ambient_owner();
        let i18n = I18n::new(build_set(), &[]).expect("engine builds");

        assert_eq!(i18n.locale(), i18n.format_locale());
    }

    /// The reactivity contract itself: a `set_locale` write wakes a live
    /// `TrackedScope` that read `.locale()` — the frame-rebuild-wake bridge
    /// `frust-reactive::tracked`'s own tests pin (see that module's doc).
    /// `frust-reactive` is a sanctioned dev-dependency for this (this
    /// module's doc's *Charter* note; `clean-signals-frust` sets the
    /// precedent) — production code above never names it.
    #[test]
    fn set_locale_wakes_a_tracked_scope_reading_locale() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let waker: FrameWaker = Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
        });
        let _rt = ReactiveRuntime::init(waker);
        let _owner = ambient_owner();

        let i18n = I18n::new(build_set(), &[locale("de")]).expect("engine builds");
        let scope = TrackedScope::new();
        scope.track(|| i18n.locale());
        assert!(!scope.is_dirty(), "fresh track starts clean");

        let before = calls.load(Ordering::SeqCst);
        i18n.set_locale(locale("en"));

        assert!(
            scope.is_dirty(),
            "a locale switch must dirty a scope that read locale()"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst) - before,
            1,
            "the switch must fire the frame waker exactly once"
        );
    }
}
