//! Spanish table (machine-drafted; pending native review).

use std::borrow::Cow;

use frust_material::{DatePickerStrings, MaterialStrings, TimePickerStringsCow};

pub fn strings() -> MaterialStrings {
    let base = MaterialStrings::ENGLISH;
    MaterialStrings {
        dialog_cancel: Cow::Borrowed("Cancelar"),
        dialog_confirm: Cow::Borrowed("Aceptar"),
        dialog_close_a11y: Cow::Borrowed("Cerrar"),
        snackbar_dismiss_a11y: Cow::Borrowed("Descartar"),
        rail_collapse_a11y: Cow::Borrowed("Contraer"),
        rail_expand_a11y: Cow::Borrowed("Expandir"),
        drawer_a11y: Cow::Borrowed("Panel de navegación"),
        search_back_a11y: Cow::Borrowed("Atrás"),
        search_clear_a11y: Cow::Borrowed("Borrar"),
        selection_clear_a11y: Cow::Borrowed("Borrar selección"),
        selection_select_all: Cow::Borrowed("Seleccionar todo"),
        selection_count_a11y: Cow::Borrowed("{count} seleccionados"),
        split_button_show_menu_a11y: Cow::Borrowed("Mostrar menú"),
        toolbar_more_options_a11y: Cow::Borrowed("Más opciones"),
        toolbar_action_fallback_a11y: Cow::Borrowed("Acción {n}"),
        fab_menu_a11y: Cow::Borrowed("Menú"),
        chip_remove_a11y: Cow::Borrowed("Quitar"),
        badge_notifications_a11y: Cow::Borrowed("Notificaciones: {count}"),
        badge_notifications_none_a11y: Cow::Borrowed("Notificaciones"),
        dropdown_hint: Cow::Borrowed("Seleccionar"),
        dropdown_empty: Cow::Borrowed("No se encontraron elementos"),
        dropdown_search_hint: Cow::Borrowed("Buscar…"),
        date_picker: DatePickerStrings {
            weekday_initials: ["D", "L", "M", "X", "J", "V", "S"],
            first_day_of_week: 1,
            help_text: "Seleccionar fecha",
            no_date_selected: "Fecha",
            cancel_label: "Cancelar",
            confirm_label: "Aceptar",
            field_label: "Introducir fecha",
            field_hint: "dd/mm/aaaa",
            error_format: "Formato no válido.",
            error_invalid: "Fuera de rango.",
            previous_month: "Mes anterior",
            next_month: "Mes siguiente",
            select_year: "Seleccionar año",
            input_mode: "Cambiar al modo de entrada de texto",
            calendar_mode: "Cambiar al modo calendario",
            ..base.date_picker
        },
        time_picker: TimePickerStringsCow {
            dial_help: Cow::Borrowed("Seleccionar hora"),
            input_help: Cow::Borrowed("Introducir hora"),
            hour_label: Cow::Borrowed("Hora"),
            minute_label: Cow::Borrowed("Minuto"),
            invalid_time: Cow::Borrowed("Introduce una hora válida"),
            am: Cow::Borrowed("a. m."),
            pm: Cow::Borrowed("p. m."),
            cancel: Cow::Borrowed("Cancelar"),
            confirm: Cow::Borrowed("Aceptar"),
            dial_mode_tooltip: Cow::Borrowed("Cambiar al modo de reloj"),
            input_mode_tooltip: Cow::Borrowed("Cambiar al modo de entrada de texto"),
            hour_mode_announcement: Cow::Borrowed("Seleccionar horas"),
            minute_mode_announcement: Cow::Borrowed("Seleccionar minutos"),
        },
    }
}
