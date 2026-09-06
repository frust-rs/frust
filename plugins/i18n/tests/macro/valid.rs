// A well-formed two-locale tree compiles, and every item the expansion
// promises — the locale set, the shared engine, and the typed keys — is
// usable from the invoking module.

frust_i18n::locales!("fixtures/valid");

fn main() {
    let plain = frust_i18n::Engine::new(locale_set().with_isolating(false))
        .expect("the embedded catalog builds");
    let chain = plain.negotiate(&["de".parse().expect("valid locale")]);

    assert_eq!(
        keys::greeting(&plain.with_chain(&chain), "Ada"),
        "Hallo, Ada!"
    );
    assert_eq!(keys::cart_items(&plain.with_chain(&chain), 5), "5 Artikel");
    // `only-en` lives in the fallback alone and still resolves.
    assert_eq!(keys::only_en(&plain.with_chain(&chain)), "English only");

    assert_eq!(engine().available_locales().len(), 2);
    assert_eq!(engine().fallback_locale().to_string(), "en");
}
