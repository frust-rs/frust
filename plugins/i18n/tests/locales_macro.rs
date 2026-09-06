//! Runtime round-trip for the `locales!` macro: what it expands to has to
//! resolve, not merely compile.
//!
//! The fixture tree under `tests/fixtures/valid` is a deliberately
//! half-translated two-locale catalog (`de` is missing `only-en` and
//! `save`), so the fallback path is exercised by the data rather than by a
//! special-cased test.

frust_i18n::locales!("tests/fixtures/valid");

use frust_i18n::{Engine, Locale, Resolve};

fn locale(tag: &str) -> Locale {
    tag.parse().expect("valid locale")
}

/// An engine with bidi isolation off, so assertions can compare plain text
/// instead of counting FSI/PDI marks — the one documented reason
/// `locale_set()` is exposed alongside `engine()`.
fn plain() -> Engine {
    Engine::new(locale_set().with_isolating(false)).expect("the embedded catalog builds")
}

#[test]
fn the_embedded_set_registers_every_locale_directory() {
    let engine = engine();

    let mut available: Vec<String> = engine
        .available_locales()
        .iter()
        .map(ToString::to_string)
        .collect();
    available.sort();

    assert_eq!(available, ["de", "en"]);
    assert_eq!(engine.fallback_locale().to_string(), "en");
}

#[test]
fn a_typed_key_formats_its_arguments() {
    let engine = plain();
    let chain = engine.negotiate(&[locale("de")]);

    assert_eq!(
        keys::greeting(&engine.with_chain(&chain), "Ada"),
        "Hallo, Ada!"
    );
}

#[test]
fn a_typed_key_takes_a_borrowed_argument() {
    let engine = plain();
    let chain = engine.negotiate(&[locale("en")]);
    // A `String` owned by this frame: the generated signature must not
    // demand `'static`.
    let name = String::from("Ada");

    assert_eq!(
        keys::greeting(&engine.with_chain(&chain), name.as_str()),
        "Hello, Ada!"
    );
}

#[test]
fn a_typed_key_selects_a_plural_category() {
    let engine = plain();
    let chain = engine.negotiate(&[locale("en")]);
    let resolver = engine.with_chain(&chain);

    assert_eq!(keys::cart_items(&resolver, 1), "one item");
    assert_eq!(keys::cart_items(&resolver, 5), "5 items");
}

#[test]
fn a_message_only_the_fallback_has_resolves_through_the_chain() {
    let engine = plain();
    let chain = engine.negotiate(&[locale("de")]);

    assert_eq!(
        chain.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["de", "en"]
    );
    assert_eq!(keys::only_en(&engine.with_chain(&chain)), "English only");
}

#[test]
fn a_typed_key_renders_itself_when_resolution_fails() {
    let engine = plain();
    // An empty chain reaches no bundle at all — the failure path every
    // generated function softens: a warn-level log plus the key itself.
    let resolver = engine.with_chain(&[]);

    assert_eq!(keys::only_en(&resolver), "only-en");
    assert_eq!(keys::greeting(&resolver, "Ada"), "greeting");
}

#[test]
fn the_generated_functions_are_plain_resolve_callers() {
    // `Resolve` is the whole coupling: anything implementing it — the
    // reactive locale handle included — drives the same generated code.
    struct Fixed;

    impl Resolve for Fixed {
        fn resolve_message(
            &self,
            key: &str,
            _args: Option<&frust_i18n::__private::fluent_bundle::FluentArgs<'_>>,
        ) -> Result<String, frust_i18n::I18nError> {
            Ok(format!("<{key}>"))
        }
    }

    assert_eq!(keys::save(&Fixed), "<save>");
}
