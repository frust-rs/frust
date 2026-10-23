//! [`MaterialStrings`] — every user-visible and accessibility string the
//! Material widgets emit, carried as a theme extension.
//!
//! The extension is attached by [`baseline`](crate::tokens::baseline) with
//! [`MaterialStrings::ENGLISH`]; a localization layer replaces it with
//! [`ThemeExtensions::insert`](frust::ThemeExtensions) (last write wins). Every
//! widget looks the extension up when its view is created (`from_build_context`,
//! the ambient theme at build time), so a provided theme relabels it on the
//! next rebuild. [`from_layout_ctx`](MaterialStrings::from_layout_ctx) and
//! [`from_paint_ctx`](MaterialStrings::from_paint_ctx) remain for a custom
//! widget that must read at layout or paint time; they see the render-root
//! theme, not a provided context. Every path ends in
//! [`MaterialStrings::resolve`], which falls back to English when the theme
//! carries no extension, so custom themes keep working unchanged.
//!
//! This crate takes no dependency on any other `frust-*` plugin (the
//! design-system charter), so the table is plain data: fields are
//! `Cow<'static, str>` so [`ENGLISH`](MaterialStrings::ENGLISH) is a `const`
//! while a runtime table can own its strings.
//!
//! ## Picker tables
//!
//! [`DatePickerStrings`] is `Copy` with `&'static str` fields, which cannot
//! hold a `Cow`; it is carried as-is (it is `const`-constructible), and
//! [`DatePickerStringsOwned`] is the owned sibling a runtime table is built
//! in, bridged with [`From`] both ways (owned to static *leaks* the strings —
//! intended once per locale table). [`TimePickerStrings`] holds `String`s and
//! so cannot be a `const`; [`TimePickerStringsCow`] is its `Cow` mirror.
//!
//! The pickers read these tables at build time when no per-widget `strings()`
//! override is set. The date picker copies the theme's `Copy` table (a few
//! pointers) and the time picker converts the `Cow` mirror once per build, so
//! no picker leaks: the only leak is the one-off owned-to-static conversion of
//! a locale table, done once when the extension is built.

use std::borrow::Cow;

use frust::authoring::{LayoutCtx, PaintCtx};
use frust::{Theme, use_context};

use crate::date_picker::DatePickerStrings;
use crate::time_picker::TimePickerStrings;

/// Owned sibling of [`DatePickerStrings`] for runtime-built tables.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatePickerStringsOwned {
    /// See [`DatePickerStrings::months`].
    pub months: [Cow<'static, str>; 12],
    /// See [`DatePickerStrings::month_abbreviations`].
    pub month_abbreviations: [Cow<'static, str>; 12],
    /// See [`DatePickerStrings::weekday_initials`].
    pub weekday_initials: [Cow<'static, str>; 7],
    /// See [`DatePickerStrings::weekday_abbreviations`].
    pub weekday_abbreviations: [Cow<'static, str>; 7],
    /// See [`DatePickerStrings::first_day_of_week`].
    pub first_day_of_week: u32,
    /// See [`DatePickerStrings::help_text`].
    pub help_text: Cow<'static, str>,
    /// See [`DatePickerStrings::no_date_selected`].
    pub no_date_selected: Cow<'static, str>,
    /// See [`DatePickerStrings::cancel_label`].
    pub cancel_label: Cow<'static, str>,
    /// See [`DatePickerStrings::confirm_label`].
    pub confirm_label: Cow<'static, str>,
    /// See [`DatePickerStrings::field_label`].
    pub field_label: Cow<'static, str>,
    /// See [`DatePickerStrings::field_hint`].
    pub field_hint: Cow<'static, str>,
    /// See [`DatePickerStrings::error_format`].
    pub error_format: Cow<'static, str>,
    /// See [`DatePickerStrings::error_invalid`].
    pub error_invalid: Cow<'static, str>,
    /// See [`DatePickerStrings::previous_month`].
    pub previous_month: Cow<'static, str>,
    /// See [`DatePickerStrings::next_month`].
    pub next_month: Cow<'static, str>,
    /// See [`DatePickerStrings::select_year`].
    pub select_year: Cow<'static, str>,
    /// See [`DatePickerStrings::input_mode`].
    pub input_mode: Cow<'static, str>,
    /// See [`DatePickerStrings::calendar_mode`].
    pub calendar_mode: Cow<'static, str>,
}

fn cows<const N: usize>(a: [&'static str; N]) -> [Cow<'static, str>; N] {
    a.map(Cow::Borrowed)
}

/// Leak an owned string to `'static`; borrowed strings pass through free.
fn leak(c: Cow<'static, str>) -> &'static str {
    match c {
        Cow::Borrowed(s) => s,
        Cow::Owned(s) => Box::leak(s.into_boxed_str()),
    }
}

fn leak_all<const N: usize>(a: [Cow<'static, str>; N]) -> [&'static str; N] {
    a.map(leak)
}

impl From<DatePickerStrings> for DatePickerStringsOwned {
    fn from(s: DatePickerStrings) -> Self {
        Self {
            months: cows(s.months),
            month_abbreviations: cows(s.month_abbreviations),
            weekday_initials: cows(s.weekday_initials),
            weekday_abbreviations: cows(s.weekday_abbreviations),
            first_day_of_week: s.first_day_of_week,
            help_text: s.help_text.into(),
            no_date_selected: s.no_date_selected.into(),
            cancel_label: s.cancel_label.into(),
            confirm_label: s.confirm_label.into(),
            field_label: s.field_label.into(),
            field_hint: s.field_hint.into(),
            error_format: s.error_format.into(),
            error_invalid: s.error_invalid.into(),
            previous_month: s.previous_month.into(),
            next_month: s.next_month.into(),
            select_year: s.select_year.into(),
            input_mode: s.input_mode.into(),
            calendar_mode: s.calendar_mode.into(),
        }
    }
}

/// Owned to static: every `Cow::Owned` string is leaked (borrowed ones are
/// not). Intended once per locale table, not per frame.
impl From<DatePickerStringsOwned> for DatePickerStrings {
    fn from(s: DatePickerStringsOwned) -> Self {
        Self {
            months: leak_all(s.months),
            month_abbreviations: leak_all(s.month_abbreviations),
            weekday_initials: leak_all(s.weekday_initials),
            weekday_abbreviations: leak_all(s.weekday_abbreviations),
            first_day_of_week: s.first_day_of_week,
            help_text: leak(s.help_text),
            no_date_selected: leak(s.no_date_selected),
            cancel_label: leak(s.cancel_label),
            confirm_label: leak(s.confirm_label),
            field_label: leak(s.field_label),
            field_hint: leak(s.field_hint),
            error_format: leak(s.error_format),
            error_invalid: leak(s.error_invalid),
            previous_month: leak(s.previous_month),
            next_month: leak(s.next_month),
            select_year: leak(s.select_year),
            input_mode: leak(s.input_mode),
            calendar_mode: leak(s.calendar_mode),
        }
    }
}

/// `Cow` mirror of [`TimePickerStrings`], so the English table is a `const`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimePickerStringsCow {
    /// See [`TimePickerStrings::dial_help`].
    pub dial_help: Cow<'static, str>,
    /// See [`TimePickerStrings::input_help`].
    pub input_help: Cow<'static, str>,
    /// See [`TimePickerStrings::hour_label`].
    pub hour_label: Cow<'static, str>,
    /// See [`TimePickerStrings::minute_label`].
    pub minute_label: Cow<'static, str>,
    /// See [`TimePickerStrings::invalid_time`].
    pub invalid_time: Cow<'static, str>,
    /// See [`TimePickerStrings::am`].
    pub am: Cow<'static, str>,
    /// See [`TimePickerStrings::pm`].
    pub pm: Cow<'static, str>,
    /// See [`TimePickerStrings::cancel`].
    pub cancel: Cow<'static, str>,
    /// See [`TimePickerStrings::confirm`].
    pub confirm: Cow<'static, str>,
    /// See [`TimePickerStrings::dial_mode_tooltip`].
    pub dial_mode_tooltip: Cow<'static, str>,
    /// See [`TimePickerStrings::input_mode_tooltip`].
    pub input_mode_tooltip: Cow<'static, str>,
    /// See [`TimePickerStrings::hour_mode_announcement`].
    pub hour_mode_announcement: Cow<'static, str>,
    /// See [`TimePickerStrings::minute_mode_announcement`].
    pub minute_mode_announcement: Cow<'static, str>,
}

impl TimePickerStringsCow {
    /// The picker's own English defaults ([`TimePickerStrings::default`]).
    pub const ENGLISH: Self = Self {
        dial_help: Cow::Borrowed("Select time"),
        input_help: Cow::Borrowed("Enter time"),
        hour_label: Cow::Borrowed("Hour"),
        minute_label: Cow::Borrowed("Minute"),
        invalid_time: Cow::Borrowed("Enter a valid time"),
        am: Cow::Borrowed("AM"),
        pm: Cow::Borrowed("PM"),
        cancel: Cow::Borrowed("Cancel"),
        confirm: Cow::Borrowed("OK"),
        dial_mode_tooltip: Cow::Borrowed("Switch to dial picker mode"),
        input_mode_tooltip: Cow::Borrowed("Switch to text input mode"),
        hour_mode_announcement: Cow::Borrowed("Select hours"),
        minute_mode_announcement: Cow::Borrowed("Select minutes"),
    };

    /// The picker-facing owned table.
    pub fn to_picker_strings(&self) -> TimePickerStrings {
        TimePickerStrings::from(self.clone())
    }
}

impl From<TimePickerStringsCow> for TimePickerStrings {
    fn from(s: TimePickerStringsCow) -> Self {
        TimePickerStrings {
            dial_help: s.dial_help.into_owned(),
            input_help: s.input_help.into_owned(),
            hour_label: s.hour_label.into_owned(),
            minute_label: s.minute_label.into_owned(),
            invalid_time: s.invalid_time.into_owned(),
            am: s.am.into_owned(),
            pm: s.pm.into_owned(),
            cancel: s.cancel.into_owned(),
            confirm: s.confirm.into_owned(),
            dial_mode_tooltip: s.dial_mode_tooltip.into_owned(),
            input_mode_tooltip: s.input_mode_tooltip.into_owned(),
            hour_mode_announcement: s.hour_mode_announcement.into_owned(),
            minute_mode_announcement: s.minute_mode_announcement.into_owned(),
        }
    }
}

impl From<TimePickerStrings> for TimePickerStringsCow {
    fn from(s: TimePickerStrings) -> Self {
        Self {
            dial_help: s.dial_help.into(),
            input_help: s.input_help.into(),
            hour_label: s.hour_label.into(),
            minute_label: s.minute_label.into(),
            invalid_time: s.invalid_time.into(),
            am: s.am.into(),
            pm: s.pm.into(),
            cancel: s.cancel.into(),
            confirm: s.confirm.into(),
            dial_mode_tooltip: s.dial_mode_tooltip.into(),
            input_mode_tooltip: s.input_mode_tooltip.into(),
            hour_mode_announcement: s.hour_mode_announcement.into(),
            minute_mode_announcement: s.minute_mode_announcement.into(),
        }
    }
}

/// Every user-visible / accessibility string of the Material widgets. Fields
/// documented with a `{count}` / `{n}` placeholder are templates; use the
/// matching method to fill them.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialStrings {
    /// Dialog cancel action.
    pub dialog_cancel: Cow<'static, str>,
    /// Dialog confirm action.
    pub dialog_confirm: Cow<'static, str>,
    /// Dialog close affordance's accessible name.
    pub dialog_close_a11y: Cow<'static, str>,
    /// Snackbar dismiss affordance's accessible name.
    pub snackbar_dismiss_a11y: Cow<'static, str>,
    /// Navigation rail "collapse" accessible name.
    pub rail_collapse_a11y: Cow<'static, str>,
    /// Navigation rail "expand" accessible name.
    pub rail_expand_a11y: Cow<'static, str>,
    /// Navigation drawer's accessible name.
    pub drawer_a11y: Cow<'static, str>,
    /// Search view back affordance's accessible name.
    pub search_back_a11y: Cow<'static, str>,
    /// Search clear affordance's accessible name.
    pub search_clear_a11y: Cow<'static, str>,
    /// Selection-mode "clear selection" accessible name.
    pub selection_clear_a11y: Cow<'static, str>,
    /// Selection-mode "select all" label.
    pub selection_select_all: Cow<'static, str>,
    /// Selection count announcement, with a `{count}` placeholder
    /// (see [`selection_count`](Self::selection_count)).
    pub selection_count_a11y: Cow<'static, str>,
    /// Split button trailing affordance's accessible name.
    pub split_button_show_menu_a11y: Cow<'static, str>,
    /// Toolbar overflow affordance's accessible name.
    pub toolbar_more_options_a11y: Cow<'static, str>,
    /// Toolbar action fallback name, with an `{n}` placeholder (1-based; see
    /// [`toolbar_action_fallback`](Self::toolbar_action_fallback)).
    pub toolbar_action_fallback_a11y: Cow<'static, str>,
    /// FAB menu trigger's accessible name.
    pub fab_menu_a11y: Cow<'static, str>,
    /// Chip remove affordance's accessible name.
    pub chip_remove_a11y: Cow<'static, str>,
    /// Badge announcement with a count, `{count}` placeholder (see
    /// [`badge_notifications`](Self::badge_notifications)).
    pub badge_notifications_a11y: Cow<'static, str>,
    /// Badge announcement without a count.
    pub badge_notifications_none_a11y: Cow<'static, str>,
    /// Dropdown placeholder when nothing is selected.
    pub dropdown_hint: Cow<'static, str>,
    /// Dropdown panel message when the filter matches nothing.
    pub dropdown_empty: Cow<'static, str>,
    /// Dropdown search field placeholder.
    pub dropdown_search_hint: Cow<'static, str>,
    /// Date picker table (`Copy`, `&'static str`; see the module docs).
    pub date_picker: DatePickerStrings,
    /// Time picker table.
    pub time_picker: TimePickerStringsCow,
}

impl MaterialStrings {
    /// The English table: every value is the literal the widgets carry today.
    pub const ENGLISH: MaterialStrings = MaterialStrings {
        dialog_cancel: Cow::Borrowed("Cancel"),
        dialog_confirm: Cow::Borrowed("OK"),
        dialog_close_a11y: Cow::Borrowed("Close"),
        snackbar_dismiss_a11y: Cow::Borrowed("Dismiss"),
        rail_collapse_a11y: Cow::Borrowed("Collapse"),
        rail_expand_a11y: Cow::Borrowed("Expand"),
        drawer_a11y: Cow::Borrowed("Navigation drawer"),
        search_back_a11y: Cow::Borrowed("Back"),
        search_clear_a11y: Cow::Borrowed("Clear"),
        selection_clear_a11y: Cow::Borrowed("Clear selection"),
        selection_select_all: Cow::Borrowed("Select all"),
        selection_count_a11y: Cow::Borrowed("{count} selected"),
        split_button_show_menu_a11y: Cow::Borrowed("Show menu"),
        toolbar_more_options_a11y: Cow::Borrowed("More options"),
        toolbar_action_fallback_a11y: Cow::Borrowed("Action {n}"),
        fab_menu_a11y: Cow::Borrowed("Menu"),
        chip_remove_a11y: Cow::Borrowed("Remove"),
        badge_notifications_a11y: Cow::Borrowed("Notifications: {count}"),
        badge_notifications_none_a11y: Cow::Borrowed("Notifications"),
        dropdown_hint: Cow::Borrowed("Select"),
        dropdown_empty: Cow::Borrowed("No items found"),
        dropdown_search_hint: Cow::Borrowed("Search…"),
        date_picker: DatePickerStrings::ENGLISH,
        time_picker: TimePickerStringsCow::ENGLISH,
    };

    /// The strings in force for the *ambient* theme, read at build time via
    /// [`use_context`]`::<Theme>()` and falling back to
    /// [`ENGLISH`](Self::ENGLISH) when no theme is provided (or it carries no
    /// extension). For labels a widget bakes into its tree during `build` /
    /// `rebuild`, where no paint/layout context exists yet.
    pub(crate) fn from_build_context() -> MaterialStrings {
        use_context::<Theme>()
            .and_then(|t| t.extension::<MaterialStrings>().cloned())
            .unwrap_or(Self::ENGLISH)
    }

    /// The strings in force for `theme`: its attached extension, else
    /// [`ENGLISH`](Self::ENGLISH).
    pub fn resolve(theme: Option<&Theme>) -> Cow<'_, MaterialStrings> {
        static ENGLISH_TABLE: MaterialStrings = MaterialStrings::ENGLISH;
        Cow::Borrowed(
            theme
                .and_then(|t| t.extension::<MaterialStrings>())
                .unwrap_or(&ENGLISH_TABLE),
        )
    }

    /// [`resolve`](Self::resolve) from a paint context's theme.
    pub fn from_paint_ctx<'a>(ctx: &'a PaintCtx<'_>) -> Cow<'a, MaterialStrings> {
        Self::resolve(Theme::from_paint_ctx(ctx))
    }

    /// [`resolve`](Self::resolve) from a layout context's theme.
    pub fn from_layout_ctx<'a>(ctx: &'a LayoutCtx<'_>) -> Cow<'a, MaterialStrings> {
        Self::resolve(Theme::from_layout_ctx(ctx))
    }

    /// [`selection_count_a11y`](Self::selection_count_a11y) with `n` filled in.
    pub fn selection_count(&self, n: usize) -> String {
        fill_count(&self.selection_count_a11y, n)
    }

    /// [`toolbar_action_fallback_a11y`](Self::toolbar_action_fallback_a11y)
    /// with the 1-based action number `n` filled in.
    pub fn toolbar_action_fallback(&self, n: usize) -> String {
        self.toolbar_action_fallback_a11y
            .replace("{n}", &n.to_string())
    }

    /// [`badge_notifications_a11y`](Self::badge_notifications_a11y) with
    /// `count` filled in.
    pub fn badge_notifications(&self, count: impl std::fmt::Display) -> String {
        fill_count(&self.badge_notifications_a11y, count)
    }
}

/// `template` with its `{count}` placeholder filled in — the one substitution
/// behind every counted announcement, so a widget that keeps a template of its
/// own (read at build time) fills it the same way.
pub(crate) fn fill_count(template: &str, count: impl std::fmt::Display) -> String {
    template.replace("{count}", &count.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::baseline;

    #[test]
    fn resolve_none_is_english() {
        assert_eq!(&*MaterialStrings::resolve(None), &MaterialStrings::ENGLISH);
    }

    #[test]
    fn resolve_baseline_is_english() {
        let theme = baseline();
        assert!(theme.extension::<MaterialStrings>().is_some());
        assert_eq!(
            &*MaterialStrings::resolve(Some(&theme)),
            &MaterialStrings::ENGLISH
        );
    }

    #[test]
    fn resolve_theme_without_extension_falls_back() {
        let theme = Theme::neutral();
        assert_eq!(
            &*MaterialStrings::resolve(Some(&theme)),
            &MaterialStrings::ENGLISH
        );
    }

    #[test]
    fn replaced_extension_resolves_to_it() {
        let mut theme = baseline();
        let mut thai = MaterialStrings::ENGLISH;
        thai.dialog_cancel = "ยกเลิก".into();
        thai.dialog_confirm = "ตกลง".into();
        thai.dropdown_empty = String::from("ไม่พบรายการ").into();
        theme.extensions.insert(thai.clone());
        let got = MaterialStrings::resolve(Some(&theme));
        assert_eq!(&*got, &thai);
        assert_eq!(got.dialog_cancel, "ยกเลิก");
        assert_eq!(got.dropdown_empty, "ไม่พบรายการ");
        assert_eq!(got.rail_expand_a11y, "Expand");
    }

    #[test]
    fn templates_fill_placeholders() {
        let s = MaterialStrings::ENGLISH;
        assert_eq!(s.selection_count(3), "3 selected");
        assert_eq!(s.toolbar_action_fallback(2), "Action 2");
        assert_eq!(s.badge_notifications(5), "Notifications: 5");
    }

    #[test]
    fn picker_bridges_round_trip() {
        let owned = DatePickerStringsOwned::from(DatePickerStrings::ENGLISH);
        assert_eq!(DatePickerStrings::from(owned), DatePickerStrings::ENGLISH);
        assert_eq!(
            MaterialStrings::ENGLISH.time_picker.to_picker_strings(),
            TimePickerStrings::default()
        );
        assert_eq!(
            TimePickerStringsCow::from(TimePickerStrings::default()),
            TimePickerStringsCow::ENGLISH
        );
    }
}
