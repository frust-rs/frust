//! German table (machine-drafted; pending native review).

use std::borrow::Cow;

use frust_material::{DatePickerStrings, MaterialStrings, TimePickerStringsCow};

pub fn strings() -> MaterialStrings {
    let base = MaterialStrings::ENGLISH;
    MaterialStrings {
        dialog_cancel: Cow::Borrowed("Abbrechen"),
        dialog_confirm: Cow::Borrowed("OK"),
        dialog_close_a11y: Cow::Borrowed("Schließen"),
        snackbar_dismiss_a11y: Cow::Borrowed("Verwerfen"),
        rail_collapse_a11y: Cow::Borrowed("Einklappen"),
        rail_expand_a11y: Cow::Borrowed("Ausklappen"),
        drawer_a11y: Cow::Borrowed("Navigationsleiste"),
        search_back_a11y: Cow::Borrowed("Zurück"),
        search_clear_a11y: Cow::Borrowed("Löschen"),
        selection_clear_a11y: Cow::Borrowed("Auswahl aufheben"),
        selection_select_all: Cow::Borrowed("Alle auswählen"),
        selection_count_a11y: Cow::Borrowed("{count} ausgewählt"),
        split_button_show_menu_a11y: Cow::Borrowed("Menü anzeigen"),
        toolbar_more_options_a11y: Cow::Borrowed("Weitere Optionen"),
        toolbar_action_fallback_a11y: Cow::Borrowed("Aktion {n}"),
        fab_menu_a11y: Cow::Borrowed("Menü"),
        chip_remove_a11y: Cow::Borrowed("Entfernen"),
        badge_notifications_a11y: Cow::Borrowed("Benachrichtigungen: {count}"),
        badge_notifications_none_a11y: Cow::Borrowed("Benachrichtigungen"),
        dropdown_hint: Cow::Borrowed("Auswählen"),
        dropdown_empty: Cow::Borrowed("Keine Einträge gefunden"),
        dropdown_search_hint: Cow::Borrowed("Suchen…"),
        date_picker: DatePickerStrings {
            weekday_initials: ["S", "M", "D", "M", "D", "F", "S"],
            first_day_of_week: 1,
            help_text: "Datum auswählen",
            no_date_selected: "Datum",
            cancel_label: "Abbrechen",
            confirm_label: "OK",
            field_label: "Datum eingeben",
            field_hint: "TT.MM.JJJJ",
            error_format: "Ungültiges Format.",
            error_invalid: "Außerhalb des Bereichs.",
            previous_month: "Vorheriger Monat",
            next_month: "Nächster Monat",
            select_year: "Jahr auswählen",
            input_mode: "Zur Texteingabe wechseln",
            calendar_mode: "Zum Kalender wechseln",
            ..base.date_picker
        },
        time_picker: TimePickerStringsCow {
            dial_help: Cow::Borrowed("Uhrzeit auswählen"),
            input_help: Cow::Borrowed("Uhrzeit eingeben"),
            hour_label: Cow::Borrowed("Stunde"),
            minute_label: Cow::Borrowed("Minute"),
            invalid_time: Cow::Borrowed("Gültige Uhrzeit eingeben"),
            am: Cow::Borrowed("AM"),
            pm: Cow::Borrowed("PM"),
            cancel: Cow::Borrowed("Abbrechen"),
            confirm: Cow::Borrowed("OK"),
            dial_mode_tooltip: Cow::Borrowed("Zur Zifferblattansicht wechseln"),
            input_mode_tooltip: Cow::Borrowed("Zur Texteingabe wechseln"),
            hour_mode_announcement: Cow::Borrowed("Stunden auswählen"),
            minute_mode_announcement: Cow::Borrowed("Minuten auswählen"),
        },
    }
}
