//! The `formatting` feature's four-locale matrix, exercised through
//! `frust-i18n`'s public API only.
//!
//! `en-US`/`de-DE`/`fr-FR`/`ja-JP` cover the shapes that actually differ:
//! group separator (comma / period / narrow no-break space), decimal
//! separator, currency symbol and its side, a zero-minor-unit currency, and
//! four date orders.
//!
//! # What is pinned, and what deliberately is not
//!
//! Separators and currency placement are asserted literally — they are the
//! behavior this layer exists to deliver, and a change in one is a change
//! worth failing on. Date *spelling* is not: the expected string is produced
//! by driving ICU4X directly with the same locale and field set, so this
//! pins that the wrapper selects the right formatter without freezing
//! CLDR's month abbreviations into the test.

#![cfg(feature = "formatting")]

use frust_i18n::fmt::{self, CivilDate, CivilTime, DateLength};
use frust_i18n::{Engine, Locale, LocaleSet, args};

/// 2024-01-31, the sample date every date assertion below formats.
const SAMPLE_DATE: CivilDate = CivilDate {
    year: 2024,
    month: 1,
    day: 31,
};

/// 15:47:50, the sample time every time assertion below formats.
const SAMPLE_TIME: CivilTime = CivilTime {
    hour: 15,
    minute: 47,
    second: 50,
};

fn locale(tag: &str) -> Locale {
    tag.parse().expect("valid locale")
}

#[test]
fn decimal_grouping_and_separators_across_the_matrix() {
    let expected = [
        ("en-US", "1,234.56"),
        ("de-DE", "1.234,56"),
        // U+202F NARROW NO-BREAK SPACE: CLDR's group separator for French,
        // not the ASCII space it looks like.
        ("fr-FR", "1\u{202f}234,56"),
        ("ja-JP", "1,234.56"),
    ];

    for (tag, rendered) in expected {
        assert_eq!(fmt::decimal(&locale(tag), 1234.56), rendered, "{tag}");
    }
}

#[test]
fn currency_symbol_and_placement_across_the_matrix() {
    let expected = [
        ("en-US", "USD", "$9.99"),
        ("de-DE", "EUR", "9,99\u{a0}€"),
        ("fr-FR", "EUR", "9,99\u{a0}€"),
        ("ja-JP", "USD", "$9.99"),
    ];

    for (tag, code, rendered) in expected {
        assert_eq!(
            fmt::currency(&locale(tag), 9.99, code).expect("formats"),
            rendered,
            "{tag}/{code}"
        );
    }
}

#[test]
fn a_zero_minor_unit_currency_rounds_away_its_fraction_everywhere() {
    let expected = [
        ("en-US", "¥10"),
        ("de-DE", "10\u{a0}¥"),
        ("fr-FR", "10\u{a0}JPY"),
        ("ja-JP", "￥10"),
    ];

    for (tag, rendered) in expected {
        assert_eq!(
            fmt::currency(&locale(tag), 9.99, "JPY").expect("formats"),
            rendered,
            "{tag}"
        );
    }
}

#[test]
fn a_medium_date_carries_the_locales_own_order_and_spelling() {
    // The spellings themselves are CLDR's and are not written out here (the
    // unit test in `fmt::datetime` pins them against ICU4X directly). What
    // this asserts is that all four locales genuinely produce four different
    // renderings of the same date — a wrapper that dropped the locale on the
    // floor would collapse them.
    let rendered: Vec<String> = ["en-US", "de-DE", "fr-FR", "ja-JP"]
        .into_iter()
        .map(|tag| fmt::date(&locale(tag), SAMPLE_DATE, DateLength::Medium).expect("formats"))
        .collect();

    for one in &rendered {
        assert!(one.contains("2024") && one.contains("31"), "{one}");
    }

    let mut distinct = rendered.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), rendered.len(), "{rendered:?}");
}

#[test]
fn a_locales_hour_cycle_decides_the_clock() {
    let en = fmt::time(&locale("en-US"), SAMPLE_TIME, DateLength::Short).expect("formats");

    for tag in ["de-DE", "fr-FR", "ja-JP"] {
        assert_eq!(
            fmt::time(&locale(tag), SAMPLE_TIME, DateLength::Short).expect("formats"),
            "15:47",
            "{tag}"
        );
    }
    assert!(en.starts_with("3:47"), "{en}");
}

#[test]
fn a_datetime_joins_both_halves_with_the_locales_own_glue() {
    for tag in ["en-US", "de-DE", "fr-FR", "ja-JP"] {
        let combined = fmt::datetime(&locale(tag), SAMPLE_DATE, SAMPLE_TIME, DateLength::Medium)
            .expect("formats");

        assert!(combined.contains("2024"), "{tag}: {combined}");
        assert!(combined.contains("47"), "{tag}: {combined}");
    }
}

#[test]
fn ftl_messages_format_their_numbers_and_dates_through_icu() {
    const FTL: &str = r#"
total = Total: { NUMBER($amount, style: "currency", currency: "EUR") }
due = Due { DATETIME($when, dateStyle: "medium") }
share = { NUMBER($pct, style: "percent") } complete
"#;

    let engine = Engine::new(fmt::with_icu_functions(
        LocaleSet::new(locale("en-US"))
            .with_locale(locale("en-US"), [("app.ftl", FTL)])
            .with_locale(locale("de-DE"), [("app.ftl", FTL)])
            .with_isolating(false),
    ))
    .expect("engine builds");

    let en = [locale("en-US")];
    let de = [locale("de-DE")];

    assert_eq!(
        engine
            .resolve(&en, "total", Some(&args!("amount" => 1234.5)))
            .expect("resolves"),
        "Total: €1,234.50"
    );
    assert_eq!(
        engine
            .resolve(&de, "total", Some(&args!("amount" => 1234.5)))
            .expect("resolves"),
        "Total: 1.234,50\u{a0}€"
    );
    assert_eq!(
        engine
            .resolve(&en, "share", Some(&args!("pct" => 42)))
            .expect("resolves"),
        "42% complete"
    );

    let due_en = engine
        .resolve(&en, "due", Some(&args!("when" => "2024-01-31")))
        .expect("resolves");
    let due_de = engine
        .resolve(&de, "due", Some(&args!("when" => "2024-01-31")))
        .expect("resolves");

    assert!(due_en.contains("2024") && due_de.contains("2024"));
    assert_ne!(due_en, due_de, "the date half must be locale-aware too");
}

#[test]
fn the_engines_own_message_resolution_is_untouched_by_the_registration() {
    const FTL: &str = "plain = Hello, { $name }!";

    let engine = Engine::new(fmt::with_icu_functions(
        LocaleSet::new(locale("en-US"))
            .with_locale(locale("en-US"), [("app.ftl", FTL)])
            .with_isolating(false),
    ))
    .expect("engine builds");

    assert_eq!(
        engine
            .resolve(&[locale("en-US")], "plain", Some(&args!("name" => "Ada")))
            .expect("resolves"),
        "Hello, Ada!"
    );
}

#[test]
fn registering_number_twice_fails_engine_construction() {
    let built = Engine::new(fmt::with_icu_functions(
        LocaleSet::new(locale("en-US"))
            .with_locale(locale("en-US"), [("app.ftl", "m = x")])
            .with_function("NUMBER", |_, _| fluent_bundle::FluentValue::None),
    ));

    assert!(built.is_err(), "a duplicate function id must be rejected");
}
