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
    AnyView, Axis, ButtonStyle, Color, Component, CrossAxisAlignment, EdgeInsets, FlexView, Get,
    Padding, RwSignal, Set, SizedBox, Theme, any, button, component, inflexible, text, use_context,
};
use frust_i18n::fmt::{self, CivilDate, CivilTime, DateLength};
use frust_i18n::{I18n, Locale, use_i18n};

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

/// Padding around the whole section column, in logical px.
const CONTENT_PAD: f64 = 16.0;

/// Gap between sections, in logical px.
const SECTION_GAP: f64 = 12.0;

/// Breathing room under the last section.
const BOTTOM_GAP: f64 = 24.0;

/// See the page-fn contract in [`crate::pages`]. Reads no
/// [`PlaygroundState`] signal — every reactive read here goes through the
/// app-scoped [`I18n`] handle ([`use_i18n`]) instead.
pub fn page(_state: &PlaygroundState) -> AnyView<PlaygroundState> {
    any(component(I18nDemoPage))
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

    fn build(&self, state: &mut I18nDemoPageState) -> AnyView<I18nDemoPageState> {
        // Provided by `crate::setup_i18n` under the real shell
        // (`PlaygroundApp::init`); [`resolve_i18n`] degrades rather than
        // panics when it is absent (a headless harness driving this page
        // directly, e.g. `tests/smoke.rs`, never runs app `init` at all) —
        // the same "context absence is a supported state" contract
        // `pages/responsive.rs`'s own module doc documents for
        // `WindowMetrics`.
        let i18n = resolve_i18n();
        // Tracked read: subscribes this rebuild to `I18n::set_locale`, which
        // is what makes every section below re-render on a language switch.
        let active = i18n.locale();

        let plural_count = state.plural_count.get();
        let theme_choice = state.theme_choice.get();
        let muted = muted();

        let children: Vec<AnyView<I18nDemoPageState>> = vec![
            any(text("i18n").size(13.0).color(accent())),
            any(text(
                "frust-i18n's whole vertical slice: system detection, live language \
                 switching, a plural, a select, typed keys, and locale-aware formatting.",
            )
            .size(11.0)
            .color(muted)),
            any(SizedBox(None, Some(SECTION_GAP))),
            section_heading("System locales"),
            any(text(system_locales_line()).size(11.0).color(muted)),
            any(SizedBox(None, Some(SECTION_GAP))),
            section_heading("Language"),
            any(text(format!("Active: {active}")).size(12.0)),
            switcher_row(&i18n, &active),
            any(SizedBox(None, Some(SECTION_GAP))),
            section_heading("Plural — cart-items"),
            plural_stepper_row(state.plural_count, plural_count),
            any(text(crate::keys::cart_items(&i18n, plural_count)).size(12.0)),
            any(SizedBox(None, Some(SECTION_GAP))),
            section_heading("Select — theme-choice"),
            theme_choice_row(state.theme_choice, theme_choice),
            any(text(crate::keys::theme_choice(&i18n, theme_choice)).size(12.0)),
            any(SizedBox(None, Some(SECTION_GAP))),
            section_heading("Typed keys"),
            any(text(crate::keys::hello(&i18n, "Ada")).size(12.0)),
            any(text(crate::keys::order_total(&i18n, 1234.5)).size(12.0)),
            any(text(crate::keys::last_visit(&i18n, "2024-01-31")).size(12.0)),
            any(SizedBox(None, Some(SECTION_GAP))),
            section_heading("Formatting matrix (frust_i18n::fmt)"),
            formatting_table(&active, muted),
            any(SizedBox(None, Some(BOTTOM_GAP))),
        ];

        any(Padding(
            EdgeInsets::all(CONTENT_PAD),
            FlexView::new(
                Axis::Vertical,
                children.into_iter().map(inflexible).collect(),
            )
            .cross_axis(CrossAxisAlignment::Start),
        ))
    }
}

/// A section's small caption heading, in the accent role.
fn section_heading(label: &str) -> AnyView<I18nDemoPageState> {
    any(text(label.to_string()).size(12.0).color(accent()))
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
fn switcher_row(i18n: &I18n, active: &Locale) -> AnyView<I18nDemoPageState> {
    let items = SWITCHER_LOCALES
        .iter()
        .map(|(tag, label)| {
            let target: Locale = tag
                .parse()
                .expect("SWITCHER_LOCALES tags are valid BCP-47 identifiers");
            let selected = target == *active;
            let handle = i18n.clone();
            inflexible(any(button(
                *label,
                move |_state: &mut I18nDemoPageState| {
                    handle.set_locale(target.clone());
                },
            )
            .style(if selected {
                ButtonStyle::Primary
            } else {
                ButtonStyle::Secondary
            })
            .small()))
        })
        .collect();
    any(FlexView::new(Axis::Horizontal, items))
}

/// The plural stepper: one button per [`PLURAL_COUNTS`] entry, writing
/// `count_signal` on press. The selected count is [`ButtonStyle::Primary`].
fn plural_stepper_row(count_signal: RwSignal<i64>, current: i64) -> AnyView<I18nDemoPageState> {
    let items = PLURAL_COUNTS
        .iter()
        .map(|&count| {
            let selected = count == current;
            inflexible(any(button(
                count.to_string(),
                move |_state: &mut I18nDemoPageState| {
                    count_signal.set(count);
                },
            )
            .style(if selected {
                ButtonStyle::Primary
            } else {
                ButtonStyle::Secondary
            })
            .small()))
        })
        .collect();
    any(FlexView::new(Axis::Horizontal, items))
}

/// The `theme-choice` select demo's row: one button per [`THEME_CHOICES`]
/// entry, writing `choice_signal` on press. The selected choice is
/// [`ButtonStyle::Primary`].
fn theme_choice_row(
    choice_signal: RwSignal<&'static str>,
    current: &'static str,
) -> AnyView<I18nDemoPageState> {
    let items = THEME_CHOICES
        .iter()
        .map(|&choice| {
            let selected = choice == current;
            inflexible(any(button(
                choice,
                move |_state: &mut I18nDemoPageState| {
                    choice_signal.set(choice);
                },
            )
            .style(if selected {
                ButtonStyle::Primary
            } else {
                ButtonStyle::Secondary
            })
            .small()))
        })
        .collect();
    any(FlexView::new(Axis::Horizontal, items))
}

/// The decimal/currency/date/time formatting matrix, rendered through the
/// **direct** `frust_i18n::fmt` API (not FTL messages) for `locale` — every
/// value is a fresh call, so switching languages re-renders locale-correct
/// output rather than a hardcoded string.
fn formatting_table(locale: &Locale, muted: Color) -> AnyView<I18nDemoPageState> {
    let mut lines = vec![format!("decimal: {}", fmt::decimal(locale, 1234.56))];

    for code in CURRENCIES {
        let rendered = fmt::currency(locale, 1234.56, code)
            .unwrap_or_else(|error| format!("<{code} error: {error}>"));
        lines.push(format!("currency {code}: {rendered}"));
    }

    let date = fmt::date(locale, SAMPLE_DATE, DateLength::Medium)
        .unwrap_or_else(|error| format!("<date error: {error}>"));
    lines.push(format!("date: {date}"));

    let time = fmt::time(locale, SAMPLE_TIME, DateLength::Short)
        .unwrap_or_else(|error| format!("<time error: {error}>"));
    lines.push(format!("time: {time}"));

    let items = lines
        .into_iter()
        .map(|line| inflexible(any(text(line).size(11.0).color(muted))))
        .collect();
    any(FlexView::new(Axis::Vertical, items).cross_axis(CrossAxisAlignment::Start))
}

/// Live-theme accent-text role (`primary`), falling back to the Material
/// baseline pre-context — the same pattern every other section page uses.
fn accent() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(Theme::m3_baseline)
        .scheme()
        .primary
}

/// A muted caption ink.
fn muted() -> Color {
    use_context::<Theme>()
        .unwrap_or_else(Theme::m3_baseline)
        .scheme()
        .on_surface_variant
}
