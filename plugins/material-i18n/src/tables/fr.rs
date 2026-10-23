//! French table (machine-drafted; pending native review).

use std::borrow::Cow;

use frust_material::{DatePickerStrings, MaterialStrings, TimePickerStringsCow};

pub fn strings() -> MaterialStrings {
    let base = MaterialStrings::ENGLISH;
    MaterialStrings {
        dialog_cancel: Cow::Borrowed("Annuler"),
        dialog_confirm: Cow::Borrowed("OK"),
        dialog_close_a11y: Cow::Borrowed("Fermer"),
        snackbar_dismiss_a11y: Cow::Borrowed("Ignorer"),
        rail_collapse_a11y: Cow::Borrowed("Réduire"),
        rail_expand_a11y: Cow::Borrowed("Développer"),
        drawer_a11y: Cow::Borrowed("Panneau de navigation"),
        search_back_a11y: Cow::Borrowed("Retour"),
        search_clear_a11y: Cow::Borrowed("Effacer"),
        selection_clear_a11y: Cow::Borrowed("Effacer la sélection"),
        selection_select_all: Cow::Borrowed("Tout sélectionner"),
        selection_count_a11y: Cow::Borrowed("{count} sélectionné(s)"),
        split_button_show_menu_a11y: Cow::Borrowed("Afficher le menu"),
        toolbar_more_options_a11y: Cow::Borrowed("Plus d’options"),
        toolbar_action_fallback_a11y: Cow::Borrowed("Action {n}"),
        fab_menu_a11y: Cow::Borrowed("Menu"),
        chip_remove_a11y: Cow::Borrowed("Supprimer"),
        badge_notifications_a11y: Cow::Borrowed("Notifications : {count}"),
        badge_notifications_none_a11y: Cow::Borrowed("Notifications"),
        dropdown_hint: Cow::Borrowed("Sélectionner"),
        dropdown_empty: Cow::Borrowed("Aucun élément trouvé"),
        dropdown_search_hint: Cow::Borrowed("Rechercher…"),
        date_picker: DatePickerStrings {
            weekday_initials: ["D", "L", "M", "M", "J", "V", "S"],
            first_day_of_week: 1,
            help_text: "Sélectionner une date",
            no_date_selected: "Date",
            cancel_label: "Annuler",
            confirm_label: "OK",
            field_label: "Saisir une date",
            field_hint: "jj/mm/aaaa",
            error_format: "Format non valide.",
            error_invalid: "Hors de la plage.",
            previous_month: "Mois précédent",
            next_month: "Mois suivant",
            select_year: "Sélectionner l’année",
            input_mode: "Passer à la saisie de texte",
            calendar_mode: "Passer au calendrier",
            ..base.date_picker
        },
        time_picker: TimePickerStringsCow {
            dial_help: Cow::Borrowed("Sélectionner l’heure"),
            input_help: Cow::Borrowed("Saisir l’heure"),
            hour_label: Cow::Borrowed("Heure"),
            minute_label: Cow::Borrowed("Minute"),
            invalid_time: Cow::Borrowed("Saisissez une heure valide"),
            am: Cow::Borrowed("AM"),
            pm: Cow::Borrowed("PM"),
            cancel: Cow::Borrowed("Annuler"),
            confirm: Cow::Borrowed("OK"),
            dial_mode_tooltip: Cow::Borrowed("Passer au mode cadran"),
            input_mode_tooltip: Cow::Borrowed("Passer à la saisie de texte"),
            hour_mode_announcement: Cow::Borrowed("Sélectionner les heures"),
            minute_mode_announcement: Cow::Borrowed("Sélectionner les minutes"),
        },
    }
}
