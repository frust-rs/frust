//! i18n section: `frust-i18n`'s whole vertical slice on one page — system
//! locale detection, live language switching (writing through the app-scoped
//! [`I18n`] handle), a CLDR plural, a Fluent select expression, compile-checked
//! typed-key call sites, and the ICU4X-backed decimal/currency/date/time
//! formatting matrix — across the three shipped locales (`locales/en`,
//! `locales/de`, `locales/ja`; English/German/Japanese).
//!
//! # Where the plugin is wired
//!
//! The `frust_i18n::locales!("locales")` compile-time bundle load and the
//! app-scoped [`I18n`] handle both live in [`crate`] (`crate::locale_set()` /
//! [`crate::keys`] and `crate::setup_i18n`, called from
//! `PlaygroundApp::init`) — see those items' own docs. This module reads the
//! already-provided handle through [`use_i18n`] ([`resolve_i18n`] is the
//! call site — see its own doc for the fallback it degrades to when nothing
//! has been provided).
//!
//! # What each `locales/*/main.ftl` message exercises
//!
//! - `hello` — plain `{ $name }` interpolation ([`crate::keys::hello`]).
//! - `cart-items` — a CLDR plural (`[0]` exact match, `[one]`, `*[other]`),
//!   driven by [`plural_stepper_row`]'s five counts
//!   ([`crate::keys::cart_items`]).
//! - `theme-choice` — a plain Fluent select expression (selecting on a
//!   literal string, not a plural category), driven by
//!   [`theme_choice_row`] ([`crate::keys::theme_choice`]).
//! - `order-total` / `last-visit` — each carries an ICU-backed
//!   `NUMBER`/`DATETIME` Fluent placeable ([`crate::keys::order_total`],
//!   [`crate::keys::last_visit`]) — registered via
//!   `frust_i18n::fmt::with_icu_functions` in `crate::setup_i18n`, since the
//!   `locales!`-generated `engine()` builds an unconfigured one.
//!
//! [`formatting_table`] exercises the *direct* `frust_i18n::fmt` API
//! (decimal, currency in EUR/USD/JPY, date, time) rather than FTL messages —
//! the two are the plugin's "two ways in, one implementation" seam
//! (`frust_i18n::fmt`'s own module doc).
//!
//! # FSI/PDI bidi isolation
//!
//! [`crate::locale_set`] leaves Unicode bidi isolation ON (the macro's
//! default — `frust_i18n::LocaleSet::with_isolating`'s own doc), so every
//! interpolated placeable below (`hello`'s `{ $name }`, `cart-items`'s
//! `{ $count }`, …) is wrapped in invisible FSI/PDI marks (`U+2068`/
//! `U+2069`). Left ON deliberately, not disabled to make a demo page's text
//! look "cleaner" — this page's own device-gate checklist is where a font
//! rendering those marks as visible tofu boxes would actually be caught (see
//! `docs/PLUGINS_DEVELOPMENT.md`); nothing here works around it.

use frust::{
    Axis, ButtonStyle, Color, Component, CrossAxisAlignment, EdgeInsets, FlexView, Get, Padding,
    RwSignal, Set, SizedBox, Theme, View, button, column, component, inflexible, row, text,
    use_context,
};
use frust_i18n::fmt::{
    self, Calendar, CivilDate, CivilTime, CompactStyle, DateLength, DateTimeOptions, ListLength,
    ListStyle, Notation, NumberOptions, RelativeDayStyle, UnitStyle,
};
use frust_i18n::{I18n, Locale, use_i18n};
use frust_material::{TimeEntry, TimeOfDay, filled_button, text_button, time_input};
use frust_material_i18n::{material_strings_for, provide_localized_strings};

use crate::PlaygroundState;

/// The three shipped locales, tag paired with its own-language display name
/// — `locales/<tag>` holds each one's `.ftl` catalog (see this module's doc).
const SWITCHER_LOCALES: [(&str, &str); 3] =
    [("en", "English"), ("de", "Deutsch"), ("ja", "日本語")];

/// The plural stepper's five counts — task-fixed at 0/1/2/5/21, the set that
/// exercises `cart-items`'s exact-`[0]`, CLDR-`[one]`, and `*[other]`
/// branches across all three locales (see `locales/*/main.ftl`).
const PLURAL_COUNTS: [i64; 5] = [0, 1, 2, 5, 21];

/// The `theme-choice` select demo's two variants — `"light"`/`"dark"` select
/// their own branch, any other value (never produced here) would fall
/// through to `*[other]`.
const THEME_CHOICES: [&str; 2] = ["light", "dark"];

/// The three ISO 4217 currencies [`formatting_table`] renders.
const CURRENCIES: [&str; 3] = ["EUR", "USD", "JPY"];

/// The sample date [`formatting_table`] formats (2024-01-31 — the same
/// sample date `plugins/i18n/tests/formatting_matrix.rs` pins).
const SAMPLE_DATE: CivilDate = CivilDate {
    year: 2024,
    month: 1,
    day: 31,
};

/// The sample time [`formatting_table`] formats.
const SAMPLE_TIME: CivilTime = CivilTime {
    hour: 15,
    minute: 47,
    second: 50,
};

/// The instant the zoned and relative-day rows read (2023-11-14 22:13:20 UTC).
const SAMPLE_INSTANT: i64 = 1_700_000_000;

/// The zone the zoned and relative-day rows read the instant in (UTC+8, so the
/// local date is already the 15th).
const SAMPLE_ZONE: &str = "Asia/Makassar";

/// One day in seconds — the "tomorrow" offset for the relative-day row.
const SECONDS_PER_DAY: i64 = 86_400;

/// A fixed amount of money in minor units (123.45 baht).
const SAMPLE_MINOR_AMOUNT: i64 = 12_345;

/// The three amenities the list row joins.
const AMENITIES: [&str; 3] = ["Wi-Fi", "Parking", "Breakfast"];

/// The tags the endonym row names, each in its own language.
const ENDONYM_TAGS: [&str; 5] = ["en", "de", "ja", "th", "vi"];

/// Padding around the whole section column, in logical px.
const CONTENT_PAD: f64 = 16.0;

/// Gap between sections, in logical px.
const SECTION_GAP: f64 = 12.0;

/// Breathing room under the last section.
const BOTTOM_GAP: f64 = 24.0;

/// See the page-fn contract in [`crate::pages`]. Reads no
/// [`PlaygroundState`] signal — every reactive read here goes through the
/// app-scoped [`I18n`] handle ([`use_i18n`]) instead.
pub fn page(_state: &PlaygroundState) -> impl View<PlaygroundState> {
    component(I18nDemoPage)
}

/// The i18n section's own [`Component`]: owns the local demo state (the
/// plural stepper's count and the select demo's choice) that has nothing to
/// do with [`PlaygroundState`] — the same page-local-`Component` shape
/// `pages/keys.rs`'s `KeysPage` uses.
struct I18nDemoPage;

/// [`I18nDemoPage`]'s retained state.
struct I18nDemoPageState {
    /// The plural stepper's current count ([`PLURAL_COUNTS`]).
    plural_count: RwSignal<i64>,
    /// The select-expression demo's current choice ([`THEME_CHOICES`]).
    theme_choice: RwSignal<&'static str>,
}

impl Component for I18nDemoPage {
    type State = I18nDemoPageState;

    fn init(&self) -> I18nDemoPageState {
        I18nDemoPageState {
            plural_count: RwSignal::new(1),
            theme_choice: RwSignal::new("light"),
        }
    }

    fn build(&self, state: &mut I18nDemoPageState) -> impl View<I18nDemoPageState> {
        // Provided by `crate::setup_i18n` under the real shell
        // (`PlaygroundApp::init`); [`resolve_i18n`] degrades rather than
        // panics when it is absent (a headless harness driving this page
        // directly, e.g. `tests/smoke.rs`, never runs app `init` at all) —
        // the same "context absence is a supported state" contract
        // `pages/responsive.rs`'s own module doc documents for
        // `WindowMetrics`.
        let i18n = resolve_i18n();
        // Tracked reads: subscribe this rebuild to `I18n::set_locale`, which
        // is what makes every section below re-render on a language switch.
        // `active` is the *message* locale (`crate::keys::*`/`i18n.t`); the
        // formatting matrix below deliberately uses `format_locale` instead
        // — see `I18n::locale`/`I18n::format_locale`'s own docs for why the
        // two can differ (a device system locale of `en-GB` negotiates a
        // message locale of `en` against this app's `en`-only bundle, but
        // `format_locale` still reports `en-GB`).
        let active = i18n.locale();
        let format_locale = i18n.format_locale();

        let plural_count = state.plural_count.get();
        let theme_choice = state.theme_choice.get();
        let muted = muted();

        let body = column()
            .child(text("i18n").size(13.0).color(accent()))
            .child(
                text(
                    "frust-i18n's whole vertical slice: system detection, live language \
                 switching, a plural, a select, typed keys, and locale-aware formatting.",
                )
                .size(11.0)
                .color(muted),
            )
            .child(SizedBox(None, Some(SECTION_GAP)))
            .child(section_heading("System locales"))
            .child(text(system_locales_line()).size(11.0).color(muted))
            .child(SizedBox(None, Some(SECTION_GAP)))
            .child(section_heading("Language"))
            .child(text(format!("Message locale: {active}")).size(12.0))
            .child(text(format!("Format locale: {format_locale}")).size(12.0))
            .child(switcher_row(&i18n, &active))
            .child(SizedBox(None, Some(SECTION_GAP)))
            .child(section_heading("Plural — cart-items"))
            .child(plural_stepper_row(state.plural_count, plural_count))
            .child(text(crate::keys::cart_items(&i18n, plural_count)).size(12.0))
            .child(SizedBox(None, Some(SECTION_GAP)))
            .child(section_heading("Select — theme-choice"))
            .child(theme_choice_row(state.theme_choice, theme_choice))
            .child(text(crate::keys::theme_choice(&i18n, theme_choice)).size(12.0))
            .child(SizedBox(None, Some(SECTION_GAP)))
            .child(section_heading("Typed keys"))
            // Without this caption the page shows two differently-sourced
            // dates with no explanation (`last-visit`'s in-message DATETIME,
            // three lines below, vs. the formatting matrix's own date row
            // further down) — see this module's doc and
            // `I18n::format_locale`'s own doc for the full mechanism.
            .child(
                text(
                    "last-visit's { DATETIME() } placeable formats at the message locale \
                 (Engine is immutable once built); the formatting matrix below uses \
                 format_locale instead — the two can render differently-regioned dates.",
                )
                .size(10.0)
                .color(muted),
            )
            .child(text(crate::keys::hello(&i18n, "Ada")).size(12.0))
            .child(text(crate::keys::order_total(&i18n, 1234.5)).size(12.0))
            .child(text(crate::keys::last_visit(&i18n, "2024-01-31")).size(12.0))
            .child(SizedBox(None, Some(SECTION_GAP)))
            .child(section_heading("Formatting matrix (frust_i18n::fmt)"))
            .child(formatting_table(&format_locale, muted))
            .child(SizedBox(None, Some(SECTION_GAP)))
            .child(section_heading("Formatting (new)"))
            .child(new_formatting_table(&format_locale, muted))
            .child(SizedBox(None, Some(SECTION_GAP)))
            .child(section_heading("Material strings"))
            .child(text(
                "A dialog action, a snackbar dismiss and an inline time picker, each reading the \
                 Material strings for the language chosen above.",
            ).size(10.0).color(muted))
            .child(component(MaterialStringsSample))
            .child(SizedBox(None, Some(BOTTOM_GAP)))
            .cross_axis(CrossAxisAlignment::Start);

        Padding(EdgeInsets::all(CONTENT_PAD), body)
    }
}

/// A section's small caption heading, in the accent role.
fn section_heading(label: &str) -> impl View<I18nDemoPageState> + use<> {
    text(label.to_string()).size(12.0).color(accent())
}

/// The app-scoped [`I18n`] handle from context, or a throwaway fallback when
/// nothing has been provided.
///
/// A real shell always has one: `crate::setup_i18n`, called from
/// `PlaygroundApp::init`, provides it before any page can build. The
/// fallback exists for a caller that mounts this page directly with no app
/// `init` at all (`tests/smoke.rs`'s headless coverage sweep, which drives
/// `pages::current` straight against a bare `PlaygroundState`) — it builds
/// its own [`frust_i18n::Engine`] from [`crate::locale_set`] with **no**
/// requested locales (negotiates to the fallback locale alone) and, unlike
/// `crate::setup_i18n`, does **not** register ICU functions — so in this
/// fallback path only, `order-total`/`last-visit`'s `NUMBER`/`DATETIME`
/// placeables render Fluent's own `NUMBER()`/`DATETIME()` placeholder rather
/// than a formatted value. A real app always goes through `setup_i18n`.
fn resolve_i18n() -> I18n {
    use_i18n().unwrap_or_else(|| {
        let engine = frust_i18n::Engine::new(crate::locale_set())
            .expect("`locales!` validated every embedded source at compile time");
        I18n::from_engine(engine, &[])
    })
}

/// The system-locale readout line: [`frust_i18n::system_locales`]'s ordered
/// preference list, or its error — re-queried on every rebuild (the
/// function's own re-query-per-call contract), so it reflects an OS
/// language-list change the very next time this page happens to rebuild.
fn system_locales_line() -> String {
    match frust_i18n::system_locales() {
        Ok(locales) => {
            let tags: Vec<String> = locales.iter().map(ToString::to_string).collect();
            format!("system_locales(): {}", tags.join(", "))
        }
        Err(error) => format!("system_locales(): {error}"),
    }
}

/// The language-switcher row: one button per [`SWITCHER_LOCALES`] entry,
/// writing through `i18n`'s handle on press — the reactive re-render proof
/// this whole page exists to demonstrate (`I18n::set_locale` re-negotiates
/// and writes the tracked locale signal every section above reads).
///
/// The active locale's button is [`ButtonStyle::Primary`]; the rest are
/// [`ButtonStyle::Secondary`].
fn switcher_row(i18n: &I18n, active: &Locale) -> impl View<I18nDemoPageState> + use<> {
    let items = SWITCHER_LOCALES
        .iter()
        .map(|(tag, label)| {
            let target: Locale = tag
                .parse()
                .expect("SWITCHER_LOCALES tags are valid BCP-47 identifiers");
            let selected = target == *active;
            let handle = i18n.clone();
            inflexible(
                button(*label, move |_state: &mut I18nDemoPageState| {
                    handle.set_locale(target.clone());
                })
                .style(if selected {
                    ButtonStyle::Primary
                } else {
                    ButtonStyle::Secondary
                })
                .small(),
            )
        })
        .collect();
    FlexView::new(Axis::Horizontal, items)
}

/// The plural stepper: one button per [`PLURAL_COUNTS`] entry, writing
/// `count_signal` on press. The selected count is [`ButtonStyle::Primary`].
fn plural_stepper_row(count_signal: RwSignal<i64>, current: i64) -> impl View<I18nDemoPageState> {
    let items = PLURAL_COUNTS
        .iter()
        .map(|&count| {
            let selected = count == current;
            inflexible(
                button(count.to_string(), move |_state: &mut I18nDemoPageState| {
                    count_signal.set(count);
                })
                .style(if selected {
                    ButtonStyle::Primary
                } else {
                    ButtonStyle::Secondary
                })
                .small(),
            )
        })
        .collect();
    FlexView::new(Axis::Horizontal, items)
}

/// The `theme-choice` select demo's row: one button per [`THEME_CHOICES`]
/// entry, writing `choice_signal` on press. The selected choice is
/// [`ButtonStyle::Primary`].
fn theme_choice_row(
    choice_signal: RwSignal<&'static str>,
    current: &'static str,
) -> impl View<I18nDemoPageState> {
    let items = THEME_CHOICES
        .iter()
        .map(|&choice| {
            let selected = choice == current;
            inflexible(
                button(choice, move |_state: &mut I18nDemoPageState| {
                    choice_signal.set(choice);
                })
                .style(if selected {
                    ButtonStyle::Primary
                } else {
                    ButtonStyle::Secondary
                })
                .small(),
            )
        })
        .collect();
    FlexView::new(Axis::Horizontal, items)
}

/// The decimal/currency/date/time formatting matrix, rendered through the
/// **direct** `frust_i18n::fmt` API (not FTL messages) for `format_locale`
/// (`I18n::format_locale`, not `I18n::locale` — CLDR formatting coverage is
/// unconstrained by which `.ftl` bundles this app shipped, so this must use
/// the caller's actual requested region rather than the negotiated message
/// locale) — every value is a fresh call, so switching languages re-renders
/// locale-correct output rather than a hardcoded string.
fn formatting_table(format_locale: &Locale, muted: Color) -> impl View<I18nDemoPageState> + use<> {
    let mut lines = vec![format!("decimal: {}", fmt::decimal(format_locale, 1234.56))];

    for code in CURRENCIES {
        let rendered = fmt::currency(format_locale, 1234.56, code)
            .unwrap_or_else(|error| format!("<{code} error: {error}>"));
        lines.push(format!("currency {code}: {rendered}"));
    }

    let date = fmt::date(format_locale, SAMPLE_DATE, DateLength::Medium)
        .unwrap_or_else(|error| format!("<date error: {error}>"));
    lines.push(format!("date: {date}"));

    let time = fmt::time(format_locale, SAMPLE_TIME, DateLength::Short)
        .unwrap_or_else(|error| format!("<time error: {error}>"));
    lines.push(format!("time: {time}"));

    let items = lines
        .into_iter()
        .map(|line| inflexible(text(line).size(11.0).color(muted)))
        .collect();
    FlexView::new(Axis::Vertical, items).cross_axis(CrossAxisAlignment::Start)
}

/// The rows for the formatting functions beyond the basic matrix: each one is
/// a fresh call with fixed inputs, so a language switch re-renders it in the
/// new locale.
///
/// The two `th-TH` date rows use a fixed locale on purpose: they show what
/// [`Calendar::Gregorian`] changes about a Thai date, which the active locale
/// would hide whenever it is not Thai.
fn new_formatting_lines(locale: &Locale) -> Vec<String> {
    fn shown(label: &str, result: Result<String, frust_i18n::I18nError>) -> String {
        format!(
            "{label}: {}",
            result.unwrap_or_else(|error| format!("<error: {error}>"))
        )
    }

    let th_th: Locale = "th-TH"
        .parse()
        .expect("`th-TH` is a valid BCP-47 identifier");
    let gregorian = DateTimeOptions {
        calendar: Some(Calendar::Gregorian),
        ..DateTimeOptions::default()
    };
    let nine = CivilTime {
        hour: 9,
        minute: 0,
        second: 0,
    };
    let eleven = CivilTime {
        hour: 11,
        minute: 0,
        second: 0,
    };
    let compact = NumberOptions {
        notation: Notation::Compact(CompactStyle::Short),
        ..NumberOptions::default()
    };
    let endonyms: Vec<String> = ENDONYM_TAGS
        .iter()
        .map(|tag| {
            let of: Locale = tag.parse().expect("ENDONYM_TAGS are valid identifiers");
            fmt::language_endonym(&of).unwrap_or_else(|error| format!("<error: {error}>"))
        })
        .collect();

    vec![
        shown(
            "currency_minor THB 12345",
            fmt::currency_minor(locale, SAMPLE_MINOR_AMOUNT, "THB"),
        ),
        shown(
            "datetime_in_zone 1700000000 Asia/Makassar",
            fmt::datetime_in_zone(locale, SAMPLE_INSTANT, SAMPLE_ZONE, DateLength::Medium),
        ),
        shown(
            "time_range 9:00-11:00",
            fmt::time_range(locale, nine, eleven, DateLength::Short),
        ),
        shown(
            "relative_day today",
            fmt::relative_day(
                locale,
                SAMPLE_INSTANT,
                SAMPLE_INSTANT,
                SAMPLE_ZONE,
                RelativeDayStyle::Long,
            ),
        ),
        shown(
            "relative_day tomorrow",
            fmt::relative_day(
                locale,
                SAMPLE_INSTANT + SECONDS_PER_DAY,
                SAMPLE_INSTANT,
                SAMPLE_ZONE,
                RelativeDayStyle::Long,
            ),
        ),
        shown(
            "duration_minutes 90",
            fmt::duration_minutes(locale, 90, UnitStyle::Long),
        ),
        shown(
            "list amenities",
            fmt::list(locale, &AMENITIES, ListStyle::And, ListLength::Wide),
        ),
        format!("endonyms: {}", endonyms.join(" / ")),
        shown(
            "date th-TH",
            fmt::date(&th_th, SAMPLE_DATE, DateLength::Medium),
        ),
        shown(
            "date th-TH Gregorian",
            fmt::date_with(&th_th, SAMPLE_DATE, DateLength::Medium, &gregorian),
        ),
        format!(
            "decimal_with 1234567 compact: {}",
            fmt::decimal_with(locale, 1_234_567.0, &compact)
        ),
    ]
}

/// [`new_formatting_lines`] as a column of muted captions.
fn new_formatting_table(
    format_locale: &Locale,
    muted: Color,
) -> impl View<I18nDemoPageState> + use<> {
    let items = new_formatting_lines(format_locale)
        .into_iter()
        .map(|line| inflexible(text(line).size(11.0).color(muted)))
        .collect();
    FlexView::new(Axis::Vertical, items).cross_axis(CrossAxisAlignment::Start)
}

/// The Material strings sample: a dialog's action buttons, a snackbar's
/// dismiss action and an inline time picker, all reading the table for the
/// active language.
///
/// A child [`Component`] so its `build` owns the tracked locale read: every
/// [`I18n::set_locale`] rebuilds it, re-provides the localized theme to the
/// widgets below (the time picker reads its field labels from it) and re-reads
/// the table for the buttons.
struct MaterialStringsSample;

impl Component for MaterialStringsSample {
    type State = ();

    fn init(&self) {}

    fn build(&self, _state: &mut ()) -> impl View<()> {
        let i18n = resolve_i18n();
        provide_localized_strings(&i18n);
        let strings = material_strings_for(&i18n.locale(), &i18n);
        let muted = muted();

        column()
            .child(text("Dialog actions").size(11.0).color(muted))
            .child(
                row()
                    .child(text_button(
                        strings.dialog_cancel.to_string(),
                        |_: &mut ()| {},
                    ))
                    .child(filled_button(
                        strings.dialog_confirm.to_string(),
                        |_: &mut ()| {},
                    )),
            )
            .child(SizedBox(None, Some(SECTION_GAP)))
            .child(text("Snackbar").size(11.0).color(muted))
            .child(row().child(text("Saved").size(12.0)).child(text_button(
                strings.snackbar_dismiss_a11y.to_string(),
                |_: &mut ()| {},
            )))
            .child(SizedBox(None, Some(SECTION_GAP)))
            .child(text("Time picker input").size(11.0).color(muted))
            .child(time_input(
                TimeEntry::from_time(TimeOfDay::new(9, 30), false),
                |_: &mut (), _entry| {},
            ))
            .cross_axis(CrossAxisAlignment::Start)
    }
}

/// Live-theme accent-text role (`primary`), falling back to the Material
/// baseline pre-context — the same pattern every other section page uses.
fn accent() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .primary
}

/// A muted caption ink.
fn muted() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(frust_material::baseline)
        .scheme()
        .on_surface_variant
}

#[cfg(test)]
mod tests {
    use std::any::Any;

    use frust::ComponentView;
    use frust_core::RenderRoot;
    use frust_i18n::{I18n, Locale, provide_i18n};
    use frust_text::TextContext;
    use kurbo::Size;
    use reactive_graph::owner::Owner;

    use super::*;

    fn locale(tag: &str) -> Locale {
        tag.parse().expect("valid locale")
    }

    /// The semantics labels of `view` laid out under an `I18n` that requested
    /// `tag`.
    fn labels_for<C: Component>(tag: &str, make: fn() -> C) -> Vec<String>
    where
        C::State: 'static,
    {
        let owner = Owner::new();
        owner.set();
        let engine =
            frust_i18n::Engine::new(frust_i18n::fmt::with_icu_functions(crate::locale_set()))
                .expect("`locales!` validated every embedded source at compile time");
        provide_i18n(I18n::from_engine(engine, &[locale(tag)]));

        let mut root: RenderRoot<(), ComponentView<C>> = RenderRoot::new();
        let mut logic = move |_: &mut ()| component(make());
        let mut state = ();
        let mut tcx = TextContext::new();
        root.rebuild(&mut logic, &mut state);
        root.layout_with_text(Size::new(400.0, 3000.0), &mut tcx as &mut dyn Any);
        root.semantics()
            .nodes
            .iter()
            .filter_map(|(_, n)| n.label().map(str::to_string))
            .collect()
    }

    fn has(labels: &[String], needle: &str) -> bool {
        labels.iter().any(|label| label.contains(needle))
    }

    /// Text widgets carry no semantics label, so the rows are checked at the
    /// string level (what the page hands each `text`), and the page itself is
    /// laid out once per locale to prove the whole tree builds.
    #[test]
    fn the_new_formatting_rows_render_in_english_and_thai() {
        let en = new_formatting_lines(&locale("en"));
        let th = new_formatting_lines(&locale("th"));

        assert_eq!(en.len(), 11);
        assert_eq!(th.len(), en.len());
        for (index, label) in [
            "currency_minor THB 12345",
            "datetime_in_zone 1700000000 Asia/Makassar",
            "time_range 9:00-11:00",
            "relative_day today",
            "relative_day tomorrow",
            "duration_minutes 90",
            "list amenities",
            "endonyms",
            "date th-TH",
            "date th-TH Gregorian",
            "decimal_with 1234567 compact",
        ]
        .iter()
        .enumerate()
        {
            for rows in [&en, &th] {
                let value = rows[index]
                    .strip_prefix(label)
                    .and_then(|rest| rest.strip_prefix(": "))
                    .unwrap_or_else(|| panic!("row {index} is not `{label}`: {:?}", rows[index]));
                assert!(
                    !value.is_empty() && !value.starts_with('<'),
                    "{label}: {value}"
                );
            }
        }

        // The rows that read the active locale differ between English and
        // Thai; the two fixed th-TH date rows and the endonyms do not (nor does
        // the short compact number, which CLDR spells the same in both).
        for index in [0, 1, 2, 3, 4, 5, 6] {
            assert_ne!(
                en[index], th[index],
                "row {index} did not follow the locale"
            );
        }
        for index in [7, 8, 9] {
            assert_eq!(en[index], th[index], "row {index} must be locale-fixed");
        }
        assert_ne!(en[8], en[9], "Gregorian must change the th-TH year");

        for tag in ["en", "th"] {
            let labels = labels_for(tag, || I18nDemoPage);
            assert!(has(&labels, "English"), "{tag}: {labels:?}");
        }
    }

    #[test]
    fn the_material_strings_sample_follows_the_locale() {
        let en = labels_for("en", || MaterialStringsSample);
        let ja = labels_for("ja", || MaterialStringsSample);

        let owner = Owner::new();
        owner.set();
        let i18n = I18n::from_engine(
            frust_i18n::Engine::new(crate::locale_set()).expect("embedded sources validate"),
            &[locale("en")],
        );
        let english = material_strings_for(&locale("en"), &i18n);
        let japanese = material_strings_for(&locale("ja"), &i18n);

        assert!(has(&en, &english.dialog_cancel), "{en:?}");
        assert!(has(&en, &english.snackbar_dismiss_a11y), "{en:?}");
        assert!(has(&ja, &japanese.dialog_cancel), "{ja:?}");
        assert!(has(&ja, &japanese.snackbar_dismiss_a11y), "{ja:?}");
        assert_ne!(english.dialog_cancel, japanese.dialog_cancel);
        assert!(!has(&ja, &english.dialog_cancel), "{ja:?}");
    }
}
