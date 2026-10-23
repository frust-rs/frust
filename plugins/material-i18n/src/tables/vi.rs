//! Vietnamese table (machine-drafted; pending native review).

use std::borrow::Cow;

use frust_material::{DatePickerStrings, MaterialStrings, TimePickerStringsCow};

pub fn strings() -> MaterialStrings {
    let base = MaterialStrings::ENGLISH;
    MaterialStrings {
        dialog_cancel: Cow::Borrowed("Hủy"),
        dialog_confirm: Cow::Borrowed("OK"),
        dialog_close_a11y: Cow::Borrowed("Đóng"),
        snackbar_dismiss_a11y: Cow::Borrowed("Bỏ qua"),
        rail_collapse_a11y: Cow::Borrowed("Thu gọn"),
        rail_expand_a11y: Cow::Borrowed("Mở rộng"),
        drawer_a11y: Cow::Borrowed("Ngăn điều hướng"),
        search_back_a11y: Cow::Borrowed("Quay lại"),
        search_clear_a11y: Cow::Borrowed("Xóa"),
        selection_clear_a11y: Cow::Borrowed("Bỏ chọn"),
        selection_select_all: Cow::Borrowed("Chọn tất cả"),
        selection_count_a11y: Cow::Borrowed("Đã chọn {count}"),
        split_button_show_menu_a11y: Cow::Borrowed("Hiện menu"),
        toolbar_more_options_a11y: Cow::Borrowed("Tùy chọn khác"),
        toolbar_action_fallback_a11y: Cow::Borrowed("Hành động {n}"),
        fab_menu_a11y: Cow::Borrowed("Menu"),
        chip_remove_a11y: Cow::Borrowed("Xóa"),
        badge_notifications_a11y: Cow::Borrowed("Thông báo: {count}"),
        badge_notifications_none_a11y: Cow::Borrowed("Thông báo"),
        dropdown_hint: Cow::Borrowed("Chọn"),
        dropdown_empty: Cow::Borrowed("Không tìm thấy mục nào"),
        dropdown_search_hint: Cow::Borrowed("Tìm kiếm…"),
        date_picker: DatePickerStrings {
            weekday_initials: ["CN", "T2", "T3", "T4", "T5", "T6", "T7"],
            first_day_of_week: 1,
            help_text: "Chọn ngày",
            no_date_selected: "Ngày",
            cancel_label: "Hủy",
            confirm_label: "OK",
            field_label: "Nhập ngày",
            field_hint: "dd/mm/yyyy",
            error_format: "Định dạng không hợp lệ.",
            error_invalid: "Ngoài phạm vi.",
            previous_month: "Tháng trước",
            next_month: "Tháng sau",
            select_year: "Chọn năm",
            input_mode: "Chuyển sang chế độ nhập văn bản",
            calendar_mode: "Chuyển sang chế độ lịch",
            ..base.date_picker
        },
        time_picker: TimePickerStringsCow {
            dial_help: Cow::Borrowed("Chọn giờ"),
            input_help: Cow::Borrowed("Nhập giờ"),
            hour_label: Cow::Borrowed("Giờ"),
            minute_label: Cow::Borrowed("Phút"),
            invalid_time: Cow::Borrowed("Nhập thời gian hợp lệ"),
            am: Cow::Borrowed("SA"),
            pm: Cow::Borrowed("CH"),
            cancel: Cow::Borrowed("Hủy"),
            confirm: Cow::Borrowed("OK"),
            dial_mode_tooltip: Cow::Borrowed("Chuyển sang chế độ quay số"),
            input_mode_tooltip: Cow::Borrowed("Chuyển sang chế độ nhập văn bản"),
            hour_mode_announcement: Cow::Borrowed("Chọn giờ"),
            minute_mode_announcement: Cow::Borrowed("Chọn phút"),
        },
    }
}
