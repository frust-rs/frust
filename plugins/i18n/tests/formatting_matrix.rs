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

use frust_i18n::fmt::{
    self, Calendar, CivilDate, CivilTime, DateLength, DateTimeOptions, HourCycle,
};
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

#[test]
fn minor_unit_money_formats_exactly_across_the_matrix() {
    // Exact ICU4X/CLDR output observed on 2.2.x; U+00A0 is CLDR's own space.
    let expected: [(&str, &str, i64, &str); 25] = [
        ("en-US", "JPY", 1000, "¥1,000"),
        ("en-US", "THB", 12345, "THB\u{a0}123.45"),
        ("en-US", "VND", 50000, "₫50,000"),
        ("en-US", "USD", -1999, "-$19.99"),
        ("en-US", "KWD", 123456, "KWD\u{a0}123.456"),
        ("th-TH", "JPY", 1000, "¥1,000"),
        ("th-TH", "THB", 12345, "฿123.45"),
        ("th-TH", "VND", 50000, "₫50,000"),
        ("th-TH", "USD", -1999, "-US$19.99"),
        ("th-TH", "KWD", 123456, "KWD\u{a0}123.456"),
        ("vi-VN", "JPY", 1000, "1.000\u{a0}¥"),
        ("vi-VN", "THB", 12345, "123,45\u{a0}฿"),
        ("vi-VN", "VND", 50000, "50.000\u{a0}₫"),
        ("vi-VN", "USD", -1999, "-19,99\u{a0}US$"),
        ("vi-VN", "KWD", 123456, "123,456\u{a0}KWD"),
        ("ja-JP", "JPY", 1000, "￥1,000"),
        ("ja-JP", "THB", 12345, "THB\u{a0}123.45"),
        ("ja-JP", "VND", 50000, "₫50,000"),
        ("ja-JP", "USD", -1999, "-$19.99"),
        ("ja-JP", "KWD", 123456, "KWD\u{a0}123.456"),
        ("es-ES", "JPY", 1000, "1000\u{a0}JPY"),
        ("es-ES", "THB", 12345, "123,45\u{a0}฿"),
        ("es-ES", "VND", 50000, "50.000\u{a0}₫"),
        ("es-ES", "USD", -1999, "-19,99\u{a0}US$"),
        ("es-ES", "KWD", 123456, "123,456\u{a0}KWD"),
    ];

    for (tag, code, minor, rendered) in expected {
        assert_eq!(
            fmt::currency_minor(&locale(tag), minor, code).expect("formats"),
            rendered,
            "{tag}/{code}/{minor}"
        );
    }
}

#[test]
fn minor_unit_digits_matches_the_currency_tables() {
    assert_eq!(fmt::minor_unit_digits("JPY").expect("digits"), 0);
    assert_eq!(fmt::minor_unit_digits("THB").expect("digits"), 2);
    assert_eq!(fmt::minor_unit_digits("KWD").expect("digits"), 3);
    assert!(fmt::minor_unit_digits("EU").is_err());
}

#[test]
fn an_ftl_minor_units_number_matches_the_direct_call() {
    const FTL: &str = r#"
price = { NUMBER($minor, style: "currency", currency: "THB", minorUnits: "true") }
"#;

    let engine = Engine::new(fmt::with_icu_functions(
        LocaleSet::new(locale("th-TH"))
            .with_locale(locale("th-TH"), [("app.ftl", FTL)])
            .with_isolating(false),
    ))
    .expect("engine builds");

    let resolved = engine
        .resolve(
            &[locale("th-TH")],
            "price",
            Some(&args!("minor" => "12345")),
        )
        .expect("resolves");

    assert_eq!(resolved, "฿123.45");
    assert_eq!(
        resolved,
        fmt::currency_minor(&locale("th-TH"), 12345, "THB").expect("formats")
    );
}

#[test]
fn currency_minor_agrees_with_the_f64_path_where_f64_is_exact() {
    // xorshift64*: deterministic, dependency-free pseudo-randomness.
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };

    let en = locale("en-US");
    for _ in 0..1000 {
        // Below 10^13 the quotient has at most 15 significant digits, so
        // f64's shortest rendering is exact; above that (up to 2^53) the f64
        // oracle itself drifts and is no longer an oracle.
        let minor = (next() % 10_000_000_000_000) as i64;
        let code = ["USD", "JPY", "KWD"][(next() % 3) as usize];
        let digits = fmt::minor_unit_digits(code).expect("digits");

        let oracle = fmt::currency(&en, minor as f64 / 10f64.powi(i32::from(digits)), code)
            .expect("formats");
        let exact = fmt::currency_minor(&en, minor, code).expect("formats");
        assert_eq!(exact, oracle, "{code} {minor}");
    }
}

// ---------------------------------------------------------------------------
// Zoned formatting (jiff offsets, ICU4X strings)
// ---------------------------------------------------------------------------

/// 2023-11-14T22:13:20Z.
const INSTANT: i64 = 1_700_000_000;

fn civil(date: (i32, u8, u8), time: (u8, u8, u8)) -> (CivilDate, CivilTime) {
    (
        CivilDate {
            year: date.0,
            month: date.1,
            day: date.2,
        },
        CivilTime {
            hour: time.0,
            minute: time.1,
            second: time.2,
        },
    )
}

#[test]
fn one_instant_reads_differently_per_zone() {
    let en = locale("en-US");
    let cases = [
        (
            "Asia/Makassar",
            8 * 3_600,
            civil((2023, 11, 15), (6, 13, 20)),
        ),
        (
            "America/New_York",
            -5 * 3_600,
            civil((2023, 11, 14), (17, 13, 20)),
        ),
        ("Europe/Berlin", 3_600, civil((2023, 11, 14), (23, 13, 20))),
    ];

    for (zone, offset, expected) in cases {
        assert_eq!(
            fmt::zone_offset_seconds(INSTANT, zone).unwrap(),
            offset,
            "{zone}"
        );
        assert_eq!(fmt::zoned_civil(INSTANT, zone).unwrap(), expected, "{zone}");
        assert_eq!(
            fmt::civil_at_offset(INSTANT, offset).unwrap(),
            expected,
            "{zone}"
        );

        // The zoned entry points are the zone-less ones over the civil pair.
        let (d, t) = expected;
        assert_eq!(
            fmt::date_in_zone(&en, INSTANT, zone, DateLength::Medium).unwrap(),
            fmt::date(&en, d, DateLength::Medium).unwrap()
        );
        assert_eq!(
            fmt::time_in_zone(&en, INSTANT, zone, DateLength::Medium).unwrap(),
            fmt::time(&en, t, DateLength::Medium).unwrap()
        );
        assert_eq!(
            fmt::datetime_in_zone(&en, INSTANT, zone, DateLength::Medium).unwrap(),
            fmt::datetime(&en, d, t, DateLength::Medium).unwrap()
        );
        assert_eq!(
            fmt::datetime_at_offset(&en, INSTANT, offset, DateLength::Medium).unwrap(),
            fmt::datetime(&en, d, t, DateLength::Medium).unwrap()
        );
    }
}

#[test]
fn zoned_en_us_strings_are_pinned_for_the_venue_zone() {
    let en = locale("en-US");

    assert_eq!(
        fmt::date_in_zone(&en, INSTANT, "Asia/Makassar", DateLength::Medium).unwrap(),
        "Nov 15, 2023"
    );
    assert_eq!(
        fmt::time_in_zone(&en, INSTANT, "Asia/Makassar", DateLength::Medium).unwrap(),
        "6:13:20\u{202f}AM"
    );
    assert_eq!(
        fmt::datetime_in_zone(&en, INSTANT, "Asia/Makassar", DateLength::Medium).unwrap(),
        "Nov 15, 2023, 6:13:20\u{202f}AM"
    );
    assert_eq!(
        fmt::time_in_zone(&en, INSTANT, "America/New_York", DateLength::Short).unwrap(),
        "5:13\u{202f}PM"
    );
    assert_eq!(
        fmt::time_in_zone(&en, INSTANT, "Europe/Berlin", DateLength::Short).unwrap(),
        "11:13\u{202f}PM"
    );
}

#[test]
fn a_dst_boundary_in_berlin_flips_the_offset_between_adjacent_seconds() {
    // 2024-03-31T01:00:00Z is when Europe/Berlin springs from +01:00 to +02:00.
    let boundary = 1_711_846_800;
    let de = locale("de-DE");

    assert_eq!(
        fmt::zone_offset_seconds(boundary - 1, "Europe/Berlin").unwrap(),
        3_600
    );
    assert_eq!(
        fmt::zone_offset_seconds(boundary, "Europe/Berlin").unwrap(),
        7_200
    );
    assert_eq!(
        fmt::zoned_civil(boundary - 1, "Europe/Berlin").unwrap(),
        civil((2024, 3, 31), (1, 59, 59))
    );
    assert_eq!(
        fmt::zoned_civil(boundary, "Europe/Berlin").unwrap(),
        civil((2024, 3, 31), (3, 0, 0))
    );
    assert_eq!(
        fmt::time_in_zone(&de, boundary - 1, "Europe/Berlin", DateLength::Medium).unwrap(),
        "01:59:59"
    );
    assert_eq!(
        fmt::time_in_zone(&de, boundary, "Europe/Berlin", DateLength::Medium).unwrap(),
        "03:00:00"
    );
}

#[test]
fn an_unknown_zone_is_a_typed_error() {
    let en = locale("en-US");

    assert!(matches!(
        fmt::zoned_civil(INSTANT, "Mars/Olympus"),
        Err(frust_i18n::I18nError::UnknownTimeZone(zone)) if zone == "Mars/Olympus"
    ));
    assert!(fmt::datetime_in_zone(&en, INSTANT, "Nope", DateLength::Short).is_err());
}

fn ftl_engine() -> Engine {
    Engine::new(fmt::with_icu_functions(
        LocaleSet::new(locale("en-US"))
            .with_locale(
                locale("en-US"),
                [(
                    "t.ftl",
                    concat!(
                        "venue = { DATETIME($t, timeStyle: \"short\", timeZone: \"Asia/Makassar\") }\n",
                        "offset = { DATETIME($t, timeStyle: \"short\", timeZone: \"-05:00\") }\n",
                        "utc = { DATETIME($t, timeStyle: \"short\") }\n",
                        "bad = { DATETIME($t, timeStyle: \"short\", timeZone: \"Nope/Zone\") }\n",
                    ),
                )],
            )
            .with_isolating(false),
    ))
    .expect("engine builds")
}

fn resolve(key: &str, instant: impl Into<fluent_bundle::FluentValue<'static>>) -> String {
    let mut args = fluent_bundle::FluentArgs::new();
    args.set("t", instant);
    ftl_engine()
        .resolve(&[locale("en-US")], key, Some(&args))
        .expect("resolves")
}

#[test]
fn ftl_datetime_honours_the_time_zone_option() {
    assert_eq!(resolve("venue", INSTANT as f64), "6:13\u{202f}AM");
    assert_eq!(resolve("offset", INSTANT as f64), "5:13\u{202f}PM");
    assert_eq!(resolve("utc", INSTANT as f64), "10:13\u{202f}PM");
    // An unknown zone is Fluent's placeholder, not an error.
    assert_eq!(resolve("bad", INSTANT as f64), "DATETIME()");
}

#[test]
fn ftl_iso_offsets_name_the_same_instant() {
    // 06:13:20+08:00 is 1700000000; rendered in the venue zone it round-trips.
    assert_eq!(
        resolve("venue", "2023-11-15T06:13:20+08:00"),
        "6:13\u{202f}AM"
    );
    assert_eq!(resolve("venue", "2023-11-14T22:13:20Z"), "6:13\u{202f}AM");
    assert_eq!(
        resolve("utc", "2023-11-15T06:13:20+08:00"),
        "10:13\u{202f}PM"
    );
}

// ---------------------------------------------------------------------------
// Ranges (hand-rolled: ICU4X 2.2 has no interval formatter)
// ---------------------------------------------------------------------------

fn t(hour: u8, minute: u8) -> CivilTime {
    CivilTime {
        hour,
        minute,
        second: 0,
    }
}

fn d(year: i32, month: u8, day: u8) -> CivilDate {
    CivilDate { year, month, day }
}

#[test]
fn time_ranges_collapse_a_shared_day_period_and_keep_a_differing_one() {
    let short = DateLength::Short;
    let range = |tag: &str, a, b| fmt::time_range(&locale(tag), a, b, short).unwrap();

    assert_eq!(range("en-US", t(9, 0), t(11, 0)), "9:00 – 11:00\u{202f}AM");
    assert_eq!(
        range("en-US", t(11, 0), t(13, 0)),
        "11:00\u{202f}AM – 1:00\u{202f}PM"
    );
    assert_eq!(range("ja-JP", t(9, 0), t(11, 0)), "9:00～11:00");
    assert_eq!(range("th-TH", t(9, 0), t(11, 0)), "09:00 – 11:00");
    assert_eq!(range("vi-VN", t(9, 0), t(11, 0)), "09:00 – 11:00");
    assert_eq!(range("es-ES", t(9, 0), t(11, 0)), "09:00 – 11:00");
    assert_eq!(range("es-ES", t(11, 0), t(13, 0)), "11:00 – 13:00");
}

#[test]
fn a_reversed_time_range_is_joined_in_the_given_order_without_panicking() {
    let en = locale("en-US");
    assert_eq!(
        fmt::time_range(&en, t(11, 0), t(9, 0), DateLength::Short).unwrap(),
        "11:00\u{202f}AM – 9:00\u{202f}AM"
    );
    assert_eq!(
        fmt::time_range(&locale("ja-JP"), t(11, 0), t(9, 0), DateLength::Short).unwrap(),
        "11:00～9:00"
    );
}

#[test]
fn a_range_across_local_midnight_in_a_zone_keeps_both_periods() {
    // 17:13 in New York on 2023-11-14; +6h is 23:13, +8h is 01:13 next day.
    let (start, end) = (INSTANT + 6 * 3_600, INSTANT + 8 * 3_600);
    let zone = "America/New_York";
    let short = DateLength::Short;

    assert_eq!(
        fmt::time_range_in_zone(&locale("en-US"), start, end, zone, short).unwrap(),
        "11:13\u{202f}PM – 1:13\u{202f}AM"
    );
    assert_eq!(
        fmt::time_range_in_zone(&locale("ja-JP"), start, end, zone, short).unwrap(),
        "23:13～1:13"
    );
    assert_eq!(
        fmt::time_range_in_zone(&locale("th-TH"), start, end, zone, short).unwrap(),
        "23:13 – 01:13"
    );
    assert_eq!(
        fmt::time_range_in_zone(&locale("en-US"), INSTANT, INSTANT + 7_200, zone, short).unwrap(),
        "5:13 – 7:13\u{202f}PM"
    );
    assert!(fmt::time_range_in_zone(&locale("en-US"), 0, 1, "Not/AZone", short).is_err());
}

#[test]
fn datetime_ranges_share_the_date_on_one_day_and_repeat_it_across_days() {
    let en = locale("en-US");
    let zone = "Asia/Makassar";
    let short = DateLength::Short;

    assert_eq!(
        fmt::datetime_range_in_zone(&en, INSTANT, INSTANT + 3_600, zone, short).unwrap(),
        "11/15/23, 6:13 – 7:13\u{202f}AM"
    );
    assert_eq!(
        fmt::datetime_range_in_zone(&en, INSTANT, INSTANT + 12 * 3_600, zone, short).unwrap(),
        "11/15/23, 6:13\u{202f}AM – 6:13\u{202f}PM"
    );
    assert_eq!(
        fmt::datetime_range_in_zone(&locale("vi-VN"), INSTANT, INSTANT + 3_600, zone, short)
            .unwrap(),
        "06:13 – 07:13 15/11/2023"
    );
    // Different local days: two full date-times.
    let across =
        fmt::datetime_range_in_zone(&en, INSTANT, INSTANT + 24 * 3_600, zone, short).unwrap();
    assert_eq!(
        across,
        "11/15/23, 6:13\u{202f}AM – 11/16/23, 6:13\u{202f}AM"
    );
}

#[test]
fn date_ranges_across_months_and_years_across_the_matrix() {
    let medium = DateLength::Medium;
    let cases = [
        (
            "en-US",
            "Mar 5, 2024 – Mar 9, 2024",
            "Mar 28, 2024 – Apr 2, 2024",
            "Dec 30, 2024 – Jan 2, 2025",
        ),
        (
            "es-ES",
            "5 mar 2024 – 9 mar 2024",
            "28 mar 2024 – 2 abr 2024",
            "30 dic 2024 – 2 ene 2025",
        ),
        (
            "ja-JP",
            "2024/03/05～2024/03/09",
            "2024/03/28～2024/04/02",
            "2024/12/30～2025/01/02",
        ),
        (
            "th-TH",
            "5 ม\u{e35}.ค. 2567 – 9 ม\u{e35}.ค. 2567",
            "28 ม\u{e35}.ค. 2567 – 2 เม.ย. 2567",
            "30 ธ.ค. 2567 – 2 ม.ค. 2568",
        ),
        (
            "vi-VN",
            "5 thg 3, 2024 – 9 thg 3, 2024",
            "28 thg 3, 2024 – 2 thg 4, 2024",
            "30 thg 12, 2024 – 2 thg 1, 2025",
        ),
    ];
    for (tag, same_month, months, years) in cases {
        let l = locale(tag);
        let range = |a, b| fmt::date_range(&l, a, b, medium).unwrap();
        assert_eq!(range(d(2024, 3, 5), d(2024, 3, 9)), same_month, "{tag}");
        assert_eq!(range(d(2024, 3, 28), d(2024, 4, 2)), months, "{tag}");
        assert_eq!(range(d(2024, 12, 30), d(2025, 1, 2)), years, "{tag}");
    }
}

// ---------------------------------------------------------------------------
// Relative days (venue-zone civil days)
// ---------------------------------------------------------------------------

/// 2023-11-15T00:00:00+08:00 (Asia/Makassar), as epoch seconds.
const MAKASSAR_MIDNIGHT: i64 = 1_699_977_600;

/// `day` days after the Makassar day of `MAKASSAR_MIDNIGHT`, at `hour:minute` local.
fn makassar(day: i64, hour: i64, minute: i64) -> i64 {
    MAKASSAR_MIDNIGHT + day * 86_400 + hour * 3_600 + minute * 60
}

fn relative(tag: &str, instant: i64, now: i64, style: fmt::RelativeDayStyle) -> String {
    fmt::relative_day(&locale(tag), instant, now, "Asia/Makassar", style).expect("relative day")
}

#[test]
fn relative_days_flip_at_the_venue_zones_midnight_in_five_locales() {
    use fmt::RelativeDayStyle::{Long, Short};

    // "now" is 23:45 local on the 15th; in UTC that is 15:45 on the same date,
    // so a UTC reading would put 00:30 local (16:30Z on the 15th) on "today".
    let now = makassar(0, 23, 45);
    let yesterday = makassar(-1, 23, 30);
    let today = makassar(0, 23, 30);
    let tomorrow = makassar(1, 0, 30);

    let expected: [(&str, fmt::RelativeDayStyle, [&str; 3]); 6] = [
        ("en", Long, ["yesterday", "today", "tomorrow"]),
        (
            "th",
            Long,
            [
                "เม\u{e37}\u{e48}อวาน",
                "ว\u{e31}นน\u{e35}\u{e49}",
                "พร\u{e38}\u{e48}งน\u{e35}\u{e49}",
            ],
        ),
        ("vi", Short, ["hôm qua", "hôm nay", "ngày mai"]),
        ("vi", Long, ["Hôm qua", "Hôm nay", "Ngày mai"]),
        ("ja", Long, ["昨日", "今日", "明日"]),
        ("es", Long, ["ayer", "hoy", "mañana"]),
    ];
    for (tag, style, [past, present, future]) in expected {
        assert_eq!(
            relative(tag, yesterday, now, style),
            past,
            "{tag} {style:?}"
        );
        assert_eq!(relative(tag, today, now, style), present, "{tag} {style:?}");
        assert_eq!(
            relative(tag, tomorrow, now, style),
            future,
            "{tag} {style:?}"
        );
    }
}

#[test]
fn relative_days_two_days_away_fall_back_to_a_medium_date() {
    let now = makassar(0, 23, 45);
    let after = makassar(2, 0, 30);
    let before = makassar(-2, 23, 30);

    assert_eq!(
        relative("en", after, now, fmt::RelativeDayStyle::Long),
        "Nov 17, 2023"
    );
    assert_eq!(
        relative("en", before, now, fmt::RelativeDayStyle::Long),
        "Nov 13, 2023"
    );
    assert_eq!(
        relative("es", after, now, fmt::RelativeDayStyle::Long),
        "17 nov 2023"
    );
    assert_eq!(
        relative("ja", before, now, fmt::RelativeDayStyle::Long),
        "2023/11/13"
    );
}

#[test]
fn relative_day_civil_and_an_unknown_zone() {
    let day = CivilDate {
        year: 2024,
        month: 3,
        day: 1,
    };
    let leap_eve = CivilDate {
        year: 2024,
        month: 2,
        day: 29,
    };
    assert_eq!(
        fmt::relative_day_civil(&locale("en"), day, leap_eve, fmt::RelativeDayStyle::Long)
            .expect("formats"),
        "tomorrow"
    );
    assert!(matches!(
        fmt::relative_day(
            &locale("en"),
            0,
            0,
            "Nope/Zone",
            fmt::RelativeDayStyle::Long
        ),
        Err(frust_i18n::I18nError::UnknownTimeZone(_))
    ));
}

#[test]
fn ftl_relative_day_round_trips_through_the_venue_zone() {
    let engine = Engine::new(fmt::with_icu_functions(
        LocaleSet::new(locale("en-US"))
            .with_locale(
                locale("en-US"),
                [(
                    "r.ftl",
                    concat!(
                        "day = { RELATIVE_DAY($t, $now, timeZone: \"Asia/Makassar\", style: \"short\") }\n",
                        "utc = { RELATIVE_DAY($t, $now) }\n",
                        "nonow = { RELATIVE_DAY($t) }\n",
                        "literal = { RELATIVE_DAY($t, now: 1700000000, timeZone: \"Asia/Makassar\") }\n",
                    ),
                )],
            )
            .with_isolating(false),
    ))
    .expect("engine builds");
    let resolve = |key: &str, t: i64, now: i64| {
        let mut args = fluent_bundle::FluentArgs::new();
        args.set("t", t as f64);
        args.set("now", now as f64);
        engine
            .resolve(&[locale("en-US")], key, Some(&args))
            .expect("resolves")
    };

    let now = makassar(0, 23, 45);
    assert_eq!(resolve("day", makassar(1, 0, 30), now), "tomorrow");
    assert_eq!(resolve("day", makassar(0, 23, 30), now), "today");
    assert_eq!(resolve("day", makassar(-1, 23, 30), now), "yesterday");
    assert_eq!(resolve("day", makassar(2, 0, 30), now), "Nov 17, 2023");
    // Read in UTC, 16:30Z and 15:45Z are the same day: "today", not "tomorrow".
    assert_eq!(resolve("utc", makassar(1, 0, 30), now), "today");
    assert_eq!(resolve("nonow", makassar(0, 1, 0), now), "RELATIVE_DAY()");
    // A literal `now:` (1700000000 = Nov 15 06:13 local) names the same day.
    assert_eq!(resolve("literal", makassar(0, 1, 0), now), "today");
}

/// `[short, narrow, long]` for 90, 60, 45 and 120 minutes, exactly as ICU4X
/// renders them. Vietnamese spells all three widths alike, Japanese long
/// equals short, and Thai minutes long equals short.
#[test]
fn duration_minutes_across_the_matrix() {
    use fmt::UnitStyle::{Long, Narrow, Short};

    #[rustfmt::skip]
    let expected: [(&str, u32, [&str; 3]); 20] = [
        ("en", 90, ["1.5 hr", "1.5h", "1.5 hours"]),
        ("en", 60, ["1 hr", "1h", "1 hour"]),
        ("en", 45, ["45 min", "45m", "45 minutes"]),
        ("en", 120, ["2 hr", "2h", "2 hours"]),
        ("th", 90, ["1.5 ชม.", "1.5ชม.", "1.5 ช\u{e31}\u{e48}วโมง"]),
        ("th", 60, ["1 ชม.", "1ชม.", "1 ช\u{e31}\u{e48}วโมง"]),
        ("th", 45, ["45 นาท\u{e35}", "45นาท\u{e35}", "45 นาท\u{e35}"]),
        ("th", 120, ["2 ชม.", "2ชม.", "2 ช\u{e31}\u{e48}วโมง"]),
        ("ja", 90, ["1.5 時間", "1.5h", "1.5 時間"]),
        ("ja", 60, ["1 時間", "1h", "1 時間"]),
        ("ja", 45, ["45 分", "45m", "45 分"]),
        ("ja", 120, ["2 時間", "2h", "2 時間"]),
        ("vi", 90, ["1,5 giờ", "1,5 giờ", "1,5 giờ"]),
        ("vi", 60, ["1 giờ", "1 giờ", "1 giờ"]),
        ("vi", 45, ["45 phút", "45 phút", "45 phút"]),
        ("vi", 120, ["2 giờ", "2 giờ", "2 giờ"]),
        ("es", 90, ["1,5 h", "1,5h", "1,5 horas"]),
        ("es", 60, ["1 h", "1h", "1 hora"]),
        ("es", 45, ["45 min", "45min", "45 minutos"]),
        ("es", 120, ["2 h", "2h", "2 horas"]),
    ];

    for (tag, minutes, strings) in expected {
        for (style, want) in [Short, Narrow, Long].into_iter().zip(strings) {
            assert_eq!(
                fmt::duration_minutes(&locale(tag), minutes, style).expect("formats"),
                want,
                "{tag} {minutes} min {style:?}"
            );
        }
    }
}

#[test]
fn units_render_the_other_units_and_reject_an_unknown_id() {
    let en = locale("en");
    assert_eq!(
        fmt::units(&en, 2.0, fmt::Unit::Kilometer, fmt::UnitStyle::Long).expect("formats"),
        "2 kilometers"
    );
    assert_eq!(
        fmt::units(&en, 1.0, fmt::Unit::Day, fmt::UnitStyle::Long).expect("formats"),
        "1 day"
    );
    assert_eq!(
        fmt::duration_minutes(&en, 100, fmt::UnitStyle::Short).expect("formats"),
        "1.7 hr"
    );
    assert!(fmt::units(&en, f64::NAN, fmt::Unit::Hour, fmt::UnitStyle::Short).is_err());

    let error = fmt::units_by_id(&en, 1.0, "duration-hour", fmt::UnitStyle::Short)
        .expect_err("not a data id");
    assert!(
        matches!(&error, frust_i18n::I18nError::UnsupportedUnit(id) if id == "duration-hour"),
        "{error:?}"
    );
}

#[test]
fn lists_join_in_each_locales_own_pattern() {
    use fmt::{ListLength::Wide, ListStyle::And};
    let three = ["Pool", "Shower", "Wi-Fi"];
    for (tag, want) in [
        ("en", "Pool, Shower, and Wi-Fi"),
        ("th", "Pool Shower และWi-Fi"),
        ("vi", "Pool, Shower và Wi-Fi"),
        ("ja", "Pool、Shower、Wi-Fi"),
        ("es", "Pool, Shower y Wi-Fi"),
    ] {
        assert_eq!(
            fmt::list(&locale(tag), &three, And, Wide).expect("formats"),
            want,
            "{tag}"
        );
    }
}

#[test]
fn two_item_and_one_item_lists_per_locale() {
    use fmt::{ListLength::Wide, ListStyle::And};
    for (tag, want) in [
        ("en", "Pool and Shower"),
        ("th", "PoolและShower"),
        ("vi", "Pool và Shower"),
        ("ja", "Pool、Shower"),
        ("es", "Pool y Shower"),
    ] {
        assert_eq!(
            fmt::list(&locale(tag), &["Pool", "Shower"], And, Wide).expect("formats"),
            want,
            "{tag}"
        );
        assert_eq!(
            fmt::list(&locale(tag), &["Pool"], And, Wide).expect("formats"),
            "Pool",
            "{tag}"
        );
    }
}

#[test]
fn short_lists_and_other_styles() {
    use fmt::{ListLength, ListStyle};
    let en = locale("en");
    assert_eq!(
        fmt::list(&en, &["Pool", "Shower"], ListStyle::And, ListLength::Wide).expect("formats"),
        "Pool and Shower"
    );
    assert_eq!(
        fmt::list(&en, &["Pool"], ListStyle::And, ListLength::Wide).expect("formats"),
        "Pool"
    );
    assert_eq!(
        fmt::list(&en, &[], ListStyle::And, ListLength::Wide).expect("formats"),
        ""
    );
    let three = ["Pool", "Shower", "Wi-Fi"];
    assert_eq!(
        fmt::list(&en, &three, ListStyle::And, ListLength::Short).expect("formats"),
        "Pool, Shower, & Wi-Fi"
    );
    assert_eq!(
        fmt::list(&en, &three, ListStyle::And, ListLength::Narrow).expect("formats"),
        "Pool, Shower, Wi-Fi"
    );
    assert_eq!(
        fmt::list(&en, &three, ListStyle::Unit, ListLength::Narrow).expect("formats"),
        "Pool Shower Wi-Fi"
    );
    assert_eq!(
        fmt::list(&en, &["a", "b", "c"], ListStyle::Or, ListLength::Wide).expect("formats"),
        "a, b, or c"
    );
}

#[test]
fn language_endonyms_and_cross_language_names() {
    for (tag, want) in [
        ("en", "English"),
        ("th", "ไทย"),
        ("vi", "Tiếng Việt"),
        ("ja", "日本語"),
        ("es", "español"),
        ("de", "Deutsch"),
        ("fr", "français"),
    ] {
        assert_eq!(
            fmt::language_endonym(&locale(tag)).expect("named"),
            want,
            "{tag}"
        );
    }
    assert_eq!(
        fmt::language_name(&locale("de"), &locale("en")).expect("named"),
        "German"
    );
}

#[test]
fn list_function_in_ftl() {
    let engine = Engine::new(fmt::with_icu_functions(
        LocaleSet::new(locale("en"))
            .with_locale(
                locale("en"),
                [(
                    "a.ftl",
                    "amenities = { LIST($a, $b, $c) }\neither = { LIST($a, $b, style: \"or\") }",
                )],
            )
            .with_isolating(false),
    ))
    .expect("engine");
    let chain = engine.negotiate(&[locale("en")]);
    let args = args!("a" => "Pool", "b" => "Shower", "c" => "Wi-Fi");
    assert_eq!(
        engine
            .resolve(&chain, "amenities", Some(&args))
            .expect("resolves"),
        "Pool, Shower, and Wi-Fi"
    );
    assert_eq!(
        engine
            .resolve(&chain, "either", Some(&args))
            .expect("resolves"),
        "Pool or Shower"
    );
}

const OCT_13: CivilDate = CivilDate {
    year: 2026,
    month: 10,
    day: 13,
};

fn calendar_option(calendar: Calendar) -> DateTimeOptions {
    DateTimeOptions {
        calendar: Some(calendar),
        hour_cycle: None,
    }
}

#[test]
fn the_buddhist_default_is_keyed_on_the_region_and_the_calendar_option_overrides_it() {
    let gregorian = calendar_option(Calendar::Gregorian);
    let rows = [
        ("th-TH", "13 ต.ค. 2569", "13 ต.ค. 2026"),
        ("en-TH", "Oct 13, 2569 BE", "Oct 13, 2026"),
        ("th", "13 ต.ค. 2026", "13 ต.ค. 2026"),
    ];

    for (tag, unset, pinned) in rows {
        assert_eq!(
            fmt::date(&locale(tag), OCT_13, DateLength::Medium).expect("formats"),
            unset,
            "{tag} unset"
        );
        assert_eq!(
            fmt::date_with(&locale(tag), OCT_13, DateLength::Medium, &gregorian).expect("formats"),
            pinned,
            "{tag} gregorian"
        );
    }
}

#[test]
fn the_hour_cycle_option_overrides_the_locales_clock() {
    let option = |hour_cycle| DateTimeOptions {
        calendar: None,
        hour_cycle: Some(hour_cycle),
    };
    let at = CivilTime {
        hour: 13,
        minute: 5,
        second: 0,
    };

    assert_eq!(
        fmt::time_with(
            &locale("en"),
            at,
            DateLength::Short,
            &option(HourCycle::H23)
        )
        .expect("formats"),
        "13:05"
    );
    let ja = fmt::time_with(
        &locale("ja"),
        at,
        DateLength::Short,
        &option(HourCycle::H12),
    )
    .expect("formats");
    assert!(ja.contains("1:05"), "{ja}");
    assert!(
        fmt::time_with(
            &locale("en"),
            at,
            DateLength::Short,
            &option(HourCycle::H24)
        )
        .is_err()
    );
}

#[test]
fn the_zoned_with_variants_honour_the_options() {
    let gregorian = calendar_option(Calendar::Gregorian);
    // 2026-10-13T12:00:00Z
    let instant = 1_791_892_800;
    let th = locale("th-TH");
    let medium = DateLength::Medium;

    assert_eq!(
        fmt::date_in_zone_with(&th, instant, "Asia/Bangkok", medium, &gregorian).unwrap(),
        "13 ต.ค. 2026"
    );
    assert_eq!(
        fmt::date_in_zone(&th, instant, "Asia/Bangkok", medium).unwrap(),
        "13 ต.ค. 2569"
    );
    let at = fmt::datetime_at_offset_with(&th, instant, 7 * 3600, medium, &gregorian).unwrap();
    assert!(at.contains("2026"), "{at}");
    let zoned = fmt::datetime_in_zone_with(&th, instant, "Asia/Bangkok", medium, &gregorian);
    assert!(zoned.unwrap().contains("2026"));
    let clock = DateTimeOptions {
        calendar: None,
        hour_cycle: Some(HourCycle::H23),
    };
    assert_eq!(
        fmt::time_in_zone_with(
            &locale("en"),
            instant,
            "Asia/Bangkok",
            DateLength::Short,
            &clock
        )
        .unwrap(),
        "19:00"
    );
}

#[test]
fn ftl_datetime_honours_calendar_and_hour_cycle() {
    const FTL: &str = "d = { DATETIME($t, dateStyle: \"medium\", calendar: \"gregory\") }\n\
                       u = { DATETIME($t, dateStyle: \"medium\") }\n\
                       c = { DATETIME($t, timeStyle: \"short\", hourCycle: \"h23\") }\n";
    let engine = Engine::new(fmt::with_icu_functions(
        LocaleSet::new(locale("th-TH"))
            .with_locale(locale("th-TH"), [("app.ftl", FTL)])
            .with_isolating(false),
    ))
    .expect("engine");
    let chain = engine.negotiate(&[locale("th-TH")]);
    let args = args!("t" => 1_791_892_800_i64);

    assert_eq!(
        engine.resolve(&chain, "d", Some(&args)).expect("d"),
        "13 ต.ค. 2026"
    );
    assert_eq!(
        engine.resolve(&chain, "u", Some(&args)).expect("u"),
        "13 ต.ค. 2569"
    );
    assert_eq!(
        engine.resolve(&chain, "c", Some(&args)).expect("c"),
        "12:00"
    );
}
