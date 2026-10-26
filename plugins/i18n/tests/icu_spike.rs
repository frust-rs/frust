//! Runtime spike pinning the ICU4X behaviour the later i18n API cards rely on.
//!
//! Research inferred these facts from source and baked data; this file executes them. Every
//! assertion is the exact string ICU4X produced when the test was written, for the five spike
//! locales `en`, `th`, `vi`, `ja`, `es`. A failure after an ICU4X or CLDR bump means a card
//! built on the old string needs a second look, not that the test should be loosened.
//!
//! Locale tags here are bare language identifiers on purpose: that is what
//! `fmt::bridge::lang_id` hands the formatters today.
//!
//! Surprises against the plan's expectations are documented on the test that observes them:
//! `th` list joining (no comma, no space after "และ"), `th` bare-language dates already being
//! Gregorian, `vi` capitalisation differing between short and long relative time, and the
//! unit ids the crate accepts.

#![cfg(feature = "formatting")]

use icu_datetime::fieldsets::YMD;
use icu_datetime::input::Date;
use icu_datetime::{DateTimeFormatter, DateTimeFormatterPreferences};
use icu_decimal::input::Decimal;
use icu_experimental::dimension::units::categorized_formatter::{
    CategorizedFormatter, CategorizedUnitsFormatterPreferences,
};
use icu_experimental::dimension::units::options::{UnitsFormatterOptions, Width};
use icu_experimental::displaynames::multi::LanguageDisplayNames;
use icu_experimental::displaynames::{DisplayNamesOptions, DisplayNamesPreferences};
use icu_experimental::measure::category::Duration;
use icu_experimental::relativetime::options::{Numeric, RelativeTimeFormatterOptions};
use icu_experimental::relativetime::{RelativeTimeFormatter, RelativeTimeFormatterPreferences};
use icu_list::options::{ListFormatterOptions, ListLength};
use icu_list::{ListFormatter, ListFormatterPreferences};
use icu_locale_core::LanguageIdentifier;
use icu_locale_core::preferences::extensions::unicode::keywords::CalendarAlgorithm;

fn id(tag: &str) -> LanguageIdentifier {
    tag.parse().expect("valid language identifier")
}

/// `format(0, 1, -1, 2)` days with `Numeric::Auto`.
fn relative_days(tag: &str, short: bool) -> Vec<String> {
    let mut options = RelativeTimeFormatterOptions::default();
    options.numeric = Some(Numeric::Auto);
    let prefs = RelativeTimeFormatterPreferences::from(&id(tag));
    let formatter = if short {
        RelativeTimeFormatter::try_new_short_day(prefs, options)
    } else {
        RelativeTimeFormatter::try_new_long_day(prefs, options)
    }
    .expect("relative time data");
    [0i8, 1, -1, 2]
        .iter()
        .map(|n| formatter.format(Decimal::from(*n)).to_string())
        .collect()
}

/// Day offsets 0, 1, -1, 2 under `Numeric::Auto`.
///
/// `en`, `ja`, `es` give the same words at short and long. `th` too, and its offset 2 is the
/// special word "มะรืนนี้" (the day after tomorrow), not a numeric "in 2 days" pattern. `vi`
/// was capitalised in places on CLDR 48 (long "Hôm nay", "Ngày kia"); CLDR 49 data is lower case
/// throughout, short and long.
#[test]
fn relativetime_day_auto() {
    let expected: [(&str, [&str; 4], [&str; 4]); 5] = [
        (
            "en",
            ["today", "tomorrow", "yesterday", "in 2 days"],
            ["today", "tomorrow", "yesterday", "in 2 days"],
        ),
        (
            "th",
            [
                "ว\u{e31}นน\u{e35}\u{e49}",
                "พร\u{e38}\u{e48}งน\u{e35}\u{e49}",
                "เม\u{e37}\u{e48}อวาน",
                "มะร\u{e37}นน\u{e35}\u{e49}",
            ],
            [
                "ว\u{e31}นน\u{e35}\u{e49}",
                "พร\u{e38}\u{e48}งน\u{e35}\u{e49}",
                "เม\u{e37}\u{e48}อวาน",
                "มะร\u{e37}นน\u{e35}\u{e49}",
            ],
        ),
        (
            "vi",
            ["hôm nay", "ngày mai", "hôm qua", "ngày kia"],
            ["hôm nay", "ngày mai", "hôm qua", "ngày kia"],
        ),
        (
            "ja",
            ["今日", "明日", "昨日", "明後日"],
            ["今日", "明日", "昨日", "明後日"],
        ),
        (
            "es",
            ["hoy", "mañana", "ayer", "pasado mañana"],
            ["hoy", "mañana", "ayer", "pasado mañana"],
        ),
    ];
    for (tag, short, long) in expected {
        assert_eq!(relative_days(tag, true), short, "{tag} short");
        assert_eq!(relative_days(tag, false), long, "{tag} long");
    }
}

fn unit(tag: &str, unit_id: &str, width: Width, value: &Decimal) -> String {
    let measure = match unit_id {
        "hour" => Duration::hour(),
        "minute" => Duration::minute(),
        other => panic!("spike covers hour and minute only, not `{other}`"),
    };
    CategorizedFormatter::<Duration>::try_new_core(
        CategorizedUnitsFormatterPreferences::from(&id(tag)),
        measure,
        UnitsFormatterOptions::from(width),
    )
    .expect("unit data")
    .format_fixed_decimal(value)
    .to_string()
}

/// `icu_experimental` 0.7 constructs units through typed categories (`Duration::hour()`), so the
/// string-id rejection this spike once pinned is covered by `units_by_id` in the formatting matrix.
///
/// Per locale, `[short, narrow, long]` for 1.5 then 90. Observed quirks: `vi` has one spelling
/// for all widths; `ja` long equals short; `th` minute long equals short.
#[test]
fn units_hour_minute() {
    let one_and_half: Decimal = "1.5".parse().unwrap();
    let ninety = Decimal::from(90i32);
    #[rustfmt::skip]
    let expected: [(&str, &str, [&str; 3], [&str; 3]); 10] = [
        ("en", "hour", ["1.5 hr", "1.5h", "1.5 hours"], ["90 hr", "90h", "90 hours"]),
        ("en", "minute", ["1.5 min", "1.5m", "1.5 minutes"], ["90 min", "90m", "90 minutes"]),
        ("th", "hour",
            ["1.5 ชม.", "1.5ชม.", "1.5 ช\u{e31}\u{e48}วโมง"],
            ["90 ชม.", "90ชม.", "90 ช\u{e31}\u{e48}วโมง"]),
        ("th", "minute",
            ["1.5 นาท\u{e35}", "1.5นาท\u{e35}", "1.5 นาท\u{e35}"],
            ["90 นาท\u{e35}", "90นาท\u{e35}", "90 นาท\u{e35}"]),
        ("vi", "hour", ["1,5 tiếng", "1,5 tiếng", "1,5 tiếng"], ["90 tiếng", "90 tiếng", "90 tiếng"]),
        ("vi", "minute", ["1,5 phút", "1,5 phút", "1,5 phút"], ["90 phút", "90 phút", "90 phút"]),
        ("ja", "hour", ["1.5 時間", "1.5h", "1.5 時間"], ["90 時間", "90h", "90 時間"]),
        ("ja", "minute", ["1.5 分", "1.5m", "1.5 分"], ["90 分", "90m", "90 分"]),
        ("es", "hour", ["1,5 h", "1,5h", "1,5 horas"], ["90 h", "90h", "90 horas"]),
        ("es", "minute", ["1,5 min", "1,5min", "1,5 minutos"], ["90 min", "90min", "90 minutos"]),
    ];
    let widths = [Width::Short, Width::Narrow, Width::Long];
    for (tag, unit_id, small, big) in expected {
        for (i, width) in widths.into_iter().enumerate() {
            assert_eq!(
                unit(tag, unit_id, width, &one_and_half),
                small[i],
                "{tag} {unit_id} {i} 1.5"
            );
            assert_eq!(
                unit(tag, unit_id, width, &ninety),
                big[i],
                "{tag} {unit_id} {i} 90"
            );
        }
    }
}

/// Wide "and" list of `Pool`, `Shower`, `Wi-Fi`.
///
/// `en` carries the serial comma as the plan expected. `th` contradicts any comma-based
/// expectation: no comma at all, a plain space between the first two items, and "และ" glued to
/// the last item with no space. `ja` uses the ideographic comma "、" with no conjunction.
#[test]
fn list_and_wide() {
    let expected = [
        ("en", "Pool, Shower, and Wi-Fi"),
        ("th", "Pool Shower และWi-Fi"),
        ("vi", "Pool, Shower và Wi-Fi"),
        ("ja", "Pool、Shower、Wi-Fi"),
        ("es", "Pool, Shower y Wi-Fi"),
    ];
    for (tag, want) in expected {
        let formatter = ListFormatter::try_new_and(
            ListFormatterPreferences::from(&id(tag)),
            ListFormatterOptions::default().with_length(ListLength::Wide),
        )
        .expect("list data");
        let got = formatter
            .format(["Pool", "Shower", "Wi-Fi"].iter())
            .to_string();
        assert_eq!(got, want, "{tag}");
    }
}

/// Endonyms: the formatter locale equals the language asked for. `es` is lower case
/// ("español"), as is CLDR's convention for Spanish language names; `en` is "English".
#[test]
fn language_display_names_endonyms() {
    let expected = [
        ("en", "English"),
        ("th", "ไทย"),
        ("vi", "Tiếng Việt"),
        ("ja", "日本語"),
        ("es", "español"),
    ];
    for (tag, want) in expected {
        let lang = id(tag);
        let names = LanguageDisplayNames::try_new(
            DisplayNamesPreferences::from(&lang),
            DisplayNamesOptions::default(),
        )
        .expect("display name data");
        assert_eq!(names.of(lang.language), Some(want), "{tag}");
    }
}

fn medium_date(tag: &str, calendar: Option<CalendarAlgorithm>) -> String {
    let mut prefs = DateTimeFormatterPreferences::from(&id(tag));
    prefs.calendar_algorithm = calendar;
    let date = Date::try_new_iso(2026, 10, 13).expect("valid date");
    DateTimeFormatter::try_new(prefs, YMD::medium())
        .expect("date data")
        .format(&date)
        .to_string()
}

/// 2026-10-13, `YMD::medium()`, calendar preference unset vs `Gregory`.
///
/// On the ICU4X 2.2 / CLDR 48 data bare `th` was Gregorian with no override ("2026"); on 2.4 /
/// CLDR 49 it resolves to Thailand's Buddhist calendar ("2569") like the region-bearing tags
/// (`en-TH`: "2569 BE"), and
/// `Some(CalendarAlgorithm::Gregory)` forces 2026 there. `frust-i18n` passes bare language
/// identifiers, so Thai users with a `th-TH` locale get Buddhist years today.
#[test]
fn thai_calendar_override() {
    assert_eq!(medium_date("th", None), "13 ต.ค. 2569");
    assert_eq!(
        medium_date("th", Some(CalendarAlgorithm::Gregory)),
        "13 ต.ค. 2026"
    );
    assert_eq!(medium_date("en-TH", None), "Oct 13, 2569 BE");
    assert_eq!(medium_date("th-TH", None), "13 ต.ค. 2569");
    assert_eq!(
        medium_date("th-TH", Some(CalendarAlgorithm::Gregory)),
        "13 ต.ค. 2026"
    );
    assert_eq!(
        medium_date("en-TH", Some(CalendarAlgorithm::Gregory)),
        "Oct 13, 2026"
    );
}
