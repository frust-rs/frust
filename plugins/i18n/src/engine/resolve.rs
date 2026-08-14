//! Fallback-chain message lookup and pattern formatting.
//!
//! # Chain walk
//!
//! The first bundle in the chain that owns the requested message *and* the
//! requested part of it (its value, or the named attribute for a `key.attr`
//! lookup) formats it; a locale with no bundle, no such message, or no such
//! attribute is skipped and the walk continues. Only when the whole chain
//! misses does this report [`I18nError::MissingMessage`], naming the key and
//! every locale tried.
//!
//! # Errors are surfaced, never swallowed
//!
//! `fluent-bundle` formats on a best-effort basis: an undefined variable or
//! a missing select default yields a placeholder string *plus* an error
//! list. Once a bundle owns the message, this module treats a non-empty
//! error list as failure — [`I18nError::MissingVariable`] when a referenced
//! `$variable` had no argument, [`I18nError::Format`] otherwise — rather
//! than returning half-resolved text; and it does **not** fall through to
//! the next locale, since a message that exists but cannot be formatted is
//! a defect in that message, not a miss.

use fluent_bundle::resolver::ResolverError;
use fluent_bundle::resolver::errors::ReferenceKind;
use fluent_bundle::{FluentArgs, FluentError};

use crate::{I18nError, Locale};

use super::bundles::{self, LocaleBundle};

/// Formats `key` from the first bundle in `chain` that has it.
///
/// `key` is a message id, optionally suffixed `.attribute` (neither a Fluent
/// message id nor an attribute name may contain a `.`, so the split is
/// unambiguous).
///
/// # Errors
///
/// [`I18nError::MissingMessage`] when no locale in `chain` has the message;
/// [`I18nError::MissingVariable`]/[`I18nError::Format`] when the owning
/// bundle failed to format it.
pub fn resolve(
    bundles: &[LocaleBundle],
    chain: &[Locale],
    key: &str,
    args: Option<&FluentArgs<'_>>,
) -> Result<String, I18nError> {
    let (id, attribute) = split_key(key);

    for locale in chain {
        let Some(entry) = bundles.iter().find(|entry| entry.locale == *locale) else {
            continue;
        };
        let Some(message) = entry.bundle.get_message(id) else {
            continue;
        };

        let pattern = match attribute {
            Some(attribute) => match message.get_attribute(attribute) {
                Some(attribute) => attribute.value(),
                None => continue,
            },
            None => match message.value() {
                Some(value) => value,
                None => continue,
            },
        };

        let mut errors = Vec::new();
        let formatted = entry.bundle.format_pattern(pattern, args, &mut errors);
        if let Some(error) = format_error(key, &errors) {
            return Err(error);
        }
        return Ok(formatted.into_owned());
    }

    Err(I18nError::MissingMessage {
        key: key.to_owned(),
        locale: tried(chain),
    })
}

/// Splits `message.attribute` into its two halves.
fn split_key(key: &str) -> (&str, Option<&str>) {
    match key.split_once('.') {
        Some((id, attribute)) => (id, Some(attribute)),
        None => (key, None),
    }
}

/// Maps a formatting run's error list onto this crate's error enum, or
/// `None` when the run was clean.
fn format_error(key: &str, errors: &[FluentError]) -> Option<I18nError> {
    let missing_variable = errors.iter().find_map(|error| match error {
        FluentError::ResolverError(ResolverError::Reference(ReferenceKind::Variable { id })) => {
            Some(id.clone())
        }
        _ => None,
    });

    if let Some(var) = missing_variable {
        return Some(I18nError::MissingVariable {
            key: key.to_owned(),
            var,
        });
    }
    if errors.is_empty() {
        return None;
    }

    Some(I18nError::Format(format!(
        "{key}: {}",
        bundles::join(errors)
    )))
}

/// Renders the locales a failed lookup walked, for the error message.
fn tried(chain: &[Locale]) -> String {
    if chain.is_empty() {
        return "(none)".to_owned();
    }
    chain
        .iter()
        .map(|locale| locale.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    fn locale(tag: &str) -> Locale {
        tag.parse().expect("valid locale")
    }

    fn bundle(tag: &str, ftl: &'static str) -> LocaleBundle {
        isolating_bundle(tag, ftl, true)
    }

    fn isolating_bundle(tag: &str, ftl: &'static str, use_isolating: bool) -> LocaleBundle {
        let locale = locale(tag);
        let bundle = bundles::build(&locale, &[("test.ftl", ftl)], use_isolating, &[])
            .expect("fixture parses");
        LocaleBundle { locale, bundle }
    }

    #[test]
    fn first_owning_bundle_in_the_chain_wins() {
        let bundles = [bundle("de", "hello = Hallo"), bundle("en", "hello = Hello")];
        let chain = [locale("de"), locale("en")];

        let resolved = resolve(&bundles, &chain, "hello", None).expect("resolves");

        assert_eq!(resolved, "Hallo");
    }

    #[test]
    fn a_key_only_the_fallback_has_resolves_there() {
        let bundles = [bundle("de", "hello = Hallo"), bundle("en", "bye = Bye")];
        let chain = [locale("de"), locale("en")];

        let resolved = resolve(&bundles, &chain, "bye", None).expect("resolves in the fallback");

        assert_eq!(resolved, "Bye");
    }

    #[test]
    fn a_locale_with_no_bundle_is_skipped() {
        let bundles = [bundle("en", "hello = Hello")];
        let chain = [locale("fr"), locale("en")];

        assert_eq!(
            resolve(&bundles, &chain, "hello", None).expect("resolves"),
            "Hello"
        );
    }

    #[test]
    fn missing_everywhere_names_the_key_and_every_locale_tried() {
        let bundles = [bundle("de", "hello = Hallo"), bundle("en", "hello = Hello")];
        let chain = [locale("de"), locale("en")];

        let error = resolve(&bundles, &chain, "nope", None).expect_err("no such message");

        match error {
            I18nError::MissingMessage { key, locale } => {
                assert_eq!(key, "nope");
                assert_eq!(locale, "de, en");
            }
            other => panic!("expected MissingMessage, got {other:?}"),
        }
    }

    #[test]
    fn interpolation_isolates_arguments_by_default() {
        let bundles = [bundle("en", "hi = Hello, { $name }!")];

        let resolved = resolve(
            &bundles,
            &[locale("en")],
            "hi",
            Some(&args!("name" => "Ada")),
        )
        .expect("resolves");

        assert_eq!(resolved, "Hello, \u{2068}Ada\u{2069}!");
    }

    #[test]
    fn isolation_can_be_switched_off() {
        let bundles = [isolating_bundle("en", "hi = Hello, { $name }!", false)];

        let resolved = resolve(
            &bundles,
            &[locale("en")],
            "hi",
            Some(&args!("name" => "Ada")),
        )
        .expect("resolves");

        assert_eq!(resolved, "Hello, Ada!");
    }

    #[test]
    fn plural_selectors_pick_the_english_categories() {
        let ftl = "\
items =
    { $count ->
        [one] one item
       *[other] { $count } items
    }";
        let bundles = [isolating_bundle("en", ftl, false)];
        let chain = [locale("en")];

        let one = resolve(&bundles, &chain, "items", Some(&args!("count" => 1))).expect("resolves");
        let other =
            resolve(&bundles, &chain, "items", Some(&args!("count" => 5))).expect("resolves");

        assert_eq!(one, "one item");
        assert_eq!(other, "5 items");
    }

    #[test]
    fn plural_selectors_pick_a_many_category_locale() {
        // Polish CLDR categories: 1 = one, 3 = few, 5 = many.
        let ftl = "\
items =
    { $count ->
        [one] jeden element
        [few] { $count } elementy
        [many] { $count } elementów
       *[other] { $count } elementu
    }";
        let bundles = [isolating_bundle("pl", ftl, false)];
        let chain = [locale("pl")];

        let one = resolve(&bundles, &chain, "items", Some(&args!("count" => 1))).expect("resolves");
        let few = resolve(&bundles, &chain, "items", Some(&args!("count" => 3))).expect("resolves");
        let many =
            resolve(&bundles, &chain, "items", Some(&args!("count" => 5))).expect("resolves");

        assert_eq!(one, "jeden element");
        assert_eq!(few, "3 elementy");
        assert_eq!(many, "5 elementów");
    }

    #[test]
    fn select_expressions_resolve_and_fall_back_to_their_default() {
        let ftl = "\
greeting =
    { $tone ->
        [formal] Good evening
       *[casual] Hey
    }";
        let bundles = [isolating_bundle("en", ftl, false)];
        let chain = [locale("en")];

        let formal = resolve(
            &bundles,
            &chain,
            "greeting",
            Some(&args!("tone" => "formal")),
        )
        .expect("resolves");
        let unknown = resolve(
            &bundles,
            &chain,
            "greeting",
            Some(&args!("tone" => "shouty")),
        )
        .expect("resolves");

        assert_eq!(formal, "Good evening");
        assert_eq!(unknown, "Hey");
    }

    #[test]
    fn attributes_resolve_through_a_dotted_key() {
        let ftl = "\
save = Save
    .tooltip = Save this document
    .shortcut = Ctrl+S";
        let bundles = [bundle("en", ftl)];
        let chain = [locale("en")];

        assert_eq!(
            resolve(&bundles, &chain, "save", None).expect("value"),
            "Save"
        );
        assert_eq!(
            resolve(&bundles, &chain, "save.tooltip", None).expect("attribute"),
            "Save this document"
        );
    }

    #[test]
    fn an_unknown_attribute_falls_through_to_the_next_locale() {
        let de = "save = Speichern\n    .tooltip = Dokument speichern";
        let en = "save = Save\n    .shortcut = Ctrl+S";
        let bundles = [bundle("de", de), bundle("en", en)];
        let chain = [locale("de"), locale("en")];

        assert_eq!(
            resolve(&bundles, &chain, "save.shortcut", None).expect("attribute in the fallback"),
            "Ctrl+S"
        );
    }

    #[test]
    fn a_value_less_message_falls_through_to_the_next_locale() {
        let de = "save =\n    .tooltip = Dokument speichern";
        let bundles = [bundle("de", de), bundle("en", "save = Save")];
        let chain = [locale("de"), locale("en")];

        assert_eq!(
            resolve(&bundles, &chain, "save", None).expect("value in the fallback"),
            "Save"
        );
    }

    #[test]
    fn an_undefined_variable_is_a_typed_error() {
        let bundles = [bundle("en", "hi = Hello, { $name }!")];

        let error = resolve(&bundles, &[locale("en")], "hi", None).expect_err("no args supplied");

        match error {
            I18nError::MissingVariable { key, var } => {
                assert_eq!(key, "hi");
                assert_eq!(var, "name");
            }
            other => panic!("expected MissingVariable, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_message_reference_is_a_format_error() {
        let bundles = [bundle("en", "hi = Hello, { missing-message }!")];

        let error = resolve(&bundles, &[locale("en")], "hi", None).expect_err("dangling reference");

        match error {
            I18nError::Format(detail) => assert!(detail.contains("missing-message"), "{detail}"),
            other => panic!("expected Format, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_chain_reports_no_locales_tried() {
        let bundles = [bundle("en", "hello = Hello")];

        let error = resolve(&bundles, &[], "hello", None).expect_err("nothing to walk");

        match error {
            I18nError::MissingMessage { locale, .. } => assert_eq!(locale, "(none)"),
            other => panic!("expected MissingMessage, got {other:?}"),
        }
    }
}
