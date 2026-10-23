//! Localized Material strings that follow the active `frust-i18n` locale.
//!
//! `frust-material` carries every user-visible and accessibility string as a
//! [`MaterialStrings`] theme extension (English by default) and, as a
//! design-system plugin, cannot depend on `frust-i18n`. This facade-tier crate
//! depends on both: it picks the table for the active locale and re-provides
//! the ambient [`Theme`](frust::Theme) with that extension swapped in.
//!
//! [`provide_localized_strings`] inside a build that tracks the locale is the
//! whole recipe. Material widgets resolve their strings from the provided
//! theme when their views are created, so each `set_locale` relabels every
//! widget below on the rebuild it triggers:
//!
//! ```ignore
//! fn build(&self, state: &mut State) -> impl View<State> {
//!     // Reads the locale signal, so every `set_locale` rebuilds this
//!     // component and re-provides the theme.
//!     frust_material_i18n::provide_localized_strings(&state.i18n);
//!     /* the app tree */
//! }
//! ```
//!
//! [`localized_theme`] is for an app that manages the shell theme itself
//! (`set_app_theme`) and wants that theme to carry the same table;
//! [`material_strings_for`] returns a locale's table directly.
//!
//! One label still reads the render-root theme at layout: the selection app
//! bar's count (`selection_count_a11y`), which an app that needs it localized
//! keeps in step by also pushing [`localized_theme`] to the shell.
//!
//! # Languages
//!
//! English, Thai, Vietnamese, Japanese, Spanish, German and French. Only the
//! English table is reviewed; the others are machine-drafted pending native
//! review (see the README). An unsupported language falls back to English.
//!
//! Month and weekday names are not hand-written: they are read from the CLDR
//! data frust-i18n already bakes in, so they are correct for every supported
//! locale by construction.

mod provide;
mod tables;

pub use provide::{localized_theme, material_strings_for, provide_localized_strings};

#[cfg(test)]
mod tests {
    use std::any::Any;

    use crate::{material_strings_for, provide_localized_strings};
    use frust::authoring::text::TextContext;
    use frust::authoring::{AnyView, Size, any};
    use frust::{Component, ComponentView, View, component};
    use frust_core::RenderRoot;
    use frust_i18n::{I18n, Locale, LocaleSet};
    use frust_material::{
        DatePickerState, MaterialDate, MaterialStrings, SnackbarController, TimeOfDay, badge,
        calendar_date_picker, fab_menu, input_chip, selection_dialog, snackbar, snackbar_host,
        time_dial,
    };
    use frust_reactive::Owner;

    fn locale(tag: &str) -> Locale {
        tag.parse().expect("valid locale")
    }

    fn i18n(start: &str) -> I18n {
        let set = LocaleSet::new(locale("en"))
            .with_locale(locale("en"), [("app.ftl", "k = en")])
            .with_locale(locale("th"), [("app.ftl", "k = th")])
            .with_locale(locale("de"), [("app.ftl", "k = de")])
            .with_locale(locale("fr"), [("app.ftl", "k = fr")]);
        I18n::new(set, &[locale(start)]).expect("engine builds")
    }

    struct Host {
        i18n: I18n,
        make: fn() -> AnyView<()>,
    }

    impl Component for Host {
        type State = ();
        fn init(&self) {}
        fn build(&self, _state: &mut ()) -> impl View<()> {
            provide_localized_strings(&self.i18n);
            (self.make)()
        }
    }

    /// Builds `make` under the bridge, switches the locale to each of `tags` in
    /// turn, and returns the semantics labels and values seen after the initial
    /// build and after each switch.
    ///
    /// Only `provide_localized_strings` is in play: the render root never
    /// receives a theme, so every string seen comes from the provided context.
    fn labels_across(i18n: &I18n, tags: &[&str], make: fn() -> AnyView<()>) -> Vec<Vec<String>> {
        let mut root: RenderRoot<(), ComponentView<Host>> = RenderRoot::new();
        let host = i18n.clone();
        let mut logic = move |_: &mut ()| {
            component(Host {
                i18n: host.clone(),
                make,
            })
        };
        let mut state = ();
        let mut tcx = TextContext::new();
        let mut snapshot = |root: &mut RenderRoot<(), ComponentView<Host>>| {
            root.layout_with_text(Size::new(400.0, 700.0), &mut tcx as &mut dyn Any);
            root.semantics()
                .nodes
                .iter()
                .flat_map(|(_, n)| [n.label(), n.value()])
                .flatten()
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        root.rebuild(&mut logic, &mut state);
        let mut seen = vec![snapshot(&mut root)];
        for tag in tags {
            i18n.set_locale(locale(tag));
            root.rebuild(&mut logic, &mut state);
            seen.push(snapshot(&mut root));
        }
        seen
    }

    fn has(labels: &[String], wanted: &str) -> bool {
        labels.iter().any(|l| l == wanted)
    }

    /// Asserts `seen` (English, Thai, English) carries `english` then `thai`
    /// then `english` again, never both at once.
    fn english_thai_english(seen: &[Vec<String>], english: &str, thai: &str) {
        assert!(has(&seen[0], english), "{:?}", seen[0]);
        assert!(!has(&seen[0], thai), "{:?}", seen[0]);
        assert!(has(&seen[1], thai), "{:?}", seen[1]);
        assert!(!has(&seen[1], english), "{:?}", seen[1]);
        assert!(has(&seen[2], english), "{:?}", seen[2]);
        assert!(!has(&seen[2], thai), "{:?}", seen[2]);
    }

    fn dialog() -> AnyView<()> {
        any(selection_dialog::<()>("Pick", vec!["A".into()]))
    }

    fn dismissible_snackbar() -> AnyView<()> {
        let controller = SnackbarController::<()>::new();
        controller.show(snackbar("Saved").dismissible());
        any(snackbar_host(&controller, frust::text("body")))
    }

    fn counted_badge() -> AnyView<()> {
        any(badge(frust::text("Inbox")).count(3))
    }

    fn closed_fab_menu() -> AnyView<()> {
        any(fab_menu(frust::text("+"), false, vec![], |_: &mut ()| {}))
    }

    fn deletable_chip() -> AnyView<()> {
        any(input_chip("Tag", |_: &mut ()| {}).on_deleted(|_: &mut ()| {}))
    }

    fn dial() -> AnyView<()> {
        any(time_dial(TimeOfDay::new(9, 30), |_: &mut (), _| {}))
    }

    fn calendar() -> AnyView<()> {
        any(calendar_date_picker(
            DatePickerState::new(None, MaterialDate::new(2026, 5, 10)),
            MaterialDate::new(2026, 5, 10),
            MaterialDate::new(2026, 5, 20),
            |_: &mut (), _| {},
        ))
    }

    #[test]
    fn extension_is_english_then_thai_after_set_locale() {
        let en = material_strings_for(&locale("en"));
        assert_eq!(en.dialog_confirm, "OK");
        assert_eq!(en.date_picker.months[0], "January");
        let th = material_strings_for(&locale("th"));
        assert_eq!(th.dialog_confirm, "ตกลง");
        assert_eq!(th.date_picker.months[4], "พฤษภาคม");
        assert_eq!(th.date_picker.months[0], "มกราคม");
    }

    #[test]
    fn dialog_confirm_label_switches_to_thai() {
        let owner = Owner::new();
        owner.set();
        let i18n = i18n("en");
        let before = labels_across(&i18n, &[], dialog);
        assert!(has(&before[0], "OK"), "{:?}", before[0]);
        // A dialog shown after the switch is built under the Thai table.
        i18n.set_locale(locale("th"));
        let after = labels_across(&i18n, &[], dialog);
        assert!(has(&after[0], "ตกลง"), "{:?}", after[0]);
        assert!(has(&after[0], "ยกเลิก"), "{:?}", after[0]);
        assert!(!has(&after[0], "OK"));
    }

    #[test]
    fn snackbar_dismiss_a11y_switches_to_thai_and_back_with_provide_only() {
        let owner = Owner::new();
        owner.set();
        let i18n = i18n("en");
        let seen = labels_across(&i18n, &["th", "en"], dismissible_snackbar);
        english_thai_english(&seen, "Dismiss", "ปิด");
    }

    #[test]
    fn badge_a11y_label_switches_to_thai_and_back_with_provide_only() {
        let owner = Owner::new();
        owner.set();
        let i18n = i18n("en");
        let seen = labels_across(&i18n, &["th", "en"], counted_badge);
        english_thai_english(&seen, "Notifications: 3", "การแจ้งเตือน: 3");
    }

    #[test]
    fn fab_menu_trigger_label_switches_to_thai_and_back_with_provide_only() {
        let owner = Owner::new();
        owner.set();
        let i18n = i18n("en");
        let seen = labels_across(&i18n, &["th", "en"], closed_fab_menu);
        english_thai_english(&seen, "Menu", "เมนู");
    }

    #[test]
    fn chip_remove_label_switches_to_thai_and_back_with_provide_only() {
        let owner = Owner::new();
        owner.set();
        let i18n = i18n("en");
        let seen = labels_across(&i18n, &["th", "en"], deletable_chip);
        english_thai_english(&seen, "Remove", "ลบ");
    }

    #[test]
    fn time_picker_am_pm_switch_to_thai() {
        let owner = Owner::new();
        owner.set();
        let i18n = i18n("en");
        let seen = labels_across(&i18n, &["th"], dial);
        assert!(has(&seen[0], "AM") && has(&seen[0], "PM"), "{:?}", seen[0]);
        assert!(
            has(&seen[1], "ก่อนเที่ยง") && has(&seen[1], "หลังเที่ยง"),
            "{:?}",
            seen[1]
        );
    }

    #[test]
    fn calendar_month_names_follow_the_locale() {
        let owner = Owner::new();
        owner.set();
        let i18n = i18n("en");
        let seen = labels_across(&i18n, &["th", "de"], calendar);
        assert!(has(&seen[0], "May 2026"), "{:?}", seen[0]);
        assert!(has(&seen[1], "พฤษภาคม 2026"), "{:?}", seen[1]);
        assert!(has(&seen[2], "Mai 2026"), "{:?}", seen[2]);
    }

    #[test]
    fn german_and_french_tables_load() {
        let de = material_strings_for(&locale("de"));
        assert_eq!(de.dialog_cancel, "Abbrechen");
        assert_eq!(de.date_picker.months[2], "März");
        assert_eq!(de.date_picker.first_day_of_week, 1);
        let fr = material_strings_for(&locale("fr"));
        assert_eq!(fr.dialog_cancel, "Annuler");
        assert_eq!(fr.date_picker.months[1], "février");
    }

    #[test]
    fn every_supported_language_fills_cldr_names() {
        for tag in ["en", "th", "vi", "ja", "es", "de", "fr"] {
            let s = material_strings_for(&locale(tag));
            let d = s.date_picker;
            assert!(d.months.iter().all(|m| !m.is_empty()), "{tag}");
            assert!(d.month_abbreviations.iter().all(|m| !m.is_empty()), "{tag}");
            assert!(
                d.weekday_abbreviations.iter().all(|m| !m.is_empty()),
                "{tag}"
            );
            if tag != "en" {
                assert_ne!(
                    d.months,
                    MaterialStrings::ENGLISH.date_picker.months,
                    "{tag}"
                );
            }
        }
    }

    #[test]
    fn region_is_ignored_and_unknown_language_is_english() {
        assert_eq!(
            material_strings_for(&locale("de-CH")),
            material_strings_for(&locale("de"))
        );
        assert_eq!(
            material_strings_for(&locale("xx")),
            MaterialStrings::ENGLISH
        );
    }

    #[test]
    fn provided_theme_without_a_base_uses_the_baseline() {
        let owner = Owner::new();
        owner.set();
        let i18n = i18n("th");
        let theme = crate::localized_theme(frust_material::baseline(), &i18n);
        assert_eq!(
            theme
                .extension::<MaterialStrings>()
                .map(|s| s.dialog_cancel.to_string()),
            Some("ยกเลิก".to_string())
        );
    }
}
