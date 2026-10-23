//! CLDR cardinal plural categories through the engine's own selector
//! resolution, for the locales GoHora ships plus the ones that exercise
//! `few`/`many`.
//!
//! Each locale resolves one message with a selector over every category and
//! a `*[other]` default, for the counts 0, 1, 2, 5 and 21 — the values that
//! separate the common rule shapes (`one` at 1 only, `one` at 0 and 1,
//! no distinctions at all, and Slavic-style `few`/`many`).

use frust_i18n::{Engine, Locale, LocaleSet, args};

const FTL: &str = "\
k =
    { $n ->
        [zero] zero
        [one] one
        [two] two
        [few] few
        [many] many
       *[other] other
    }";

fn locale(tag: &str) -> Locale {
    tag.parse().expect("valid locale")
}

fn categories(tag: &str) -> Vec<String> {
    let engine =
        Engine::new(LocaleSet::new(locale(tag)).with_locale(locale(tag), [("p.ftl", FTL)]))
            .expect("engine builds");

    [0, 1, 2, 5, 21]
        .into_iter()
        .map(|n| {
            engine
                .resolve(&[locale(tag)], "k", Some(&args!("n" => n)))
                .expect("resolves")
        })
        .collect()
}

#[test]
fn cardinal_categories_for_0_1_2_5_and_21_in_eight_locales() {
    // Counts:                0        1        2        5        21
    let rows = [
        ("en", ["other", "one", "other", "other", "other"]),
        ("de", ["other", "one", "other", "other", "other"]),
        // French counts 0 as `one`.
        ("fr", ["one", "one", "other", "other", "other"]),
        ("es", ["other", "one", "other", "other", "other"]),
        // No grammatical number: every count is `other`.
        ("ja", ["other", "other", "other", "other", "other"]),
        ("th", ["other", "other", "other", "other", "other"]),
        ("vi", ["other", "other", "other", "other", "other"]),
        // Polish: 0, 5 and 21 are `many`, 2 is `few`.
        ("pl", ["many", "one", "few", "many", "many"]),
    ];

    for (tag, expected) in rows {
        assert_eq!(categories(tag), expected, "{tag}");
    }
}

#[test]
fn a_region_tag_uses_its_languages_rules() {
    assert_eq!(categories("es-ES"), categories("es"));
    assert_eq!(categories("th-TH"), categories("th"));
    assert_eq!(categories("vi-VN"), categories("vi"));
}
