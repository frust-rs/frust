//! Thai table (machine-drafted; pending native review).

use std::borrow::Cow;

use frust_material::{DatePickerStrings, MaterialStrings, TimePickerStringsCow};

pub fn strings() -> MaterialStrings {
    let base = MaterialStrings::ENGLISH;
    MaterialStrings {
        dialog_cancel: Cow::Borrowed("ยกเลิก"),
        dialog_confirm: Cow::Borrowed("ตกลง"),
        dialog_close_a11y: Cow::Borrowed("ปิด"),
        snackbar_dismiss_a11y: Cow::Borrowed("ปิด"),
        rail_collapse_a11y: Cow::Borrowed("ยุบ"),
        rail_expand_a11y: Cow::Borrowed("ขยาย"),
        drawer_a11y: Cow::Borrowed("เมนูนำทาง"),
        search_back_a11y: Cow::Borrowed("ย้อนกลับ"),
        search_clear_a11y: Cow::Borrowed("ล้าง"),
        selection_clear_a11y: Cow::Borrowed("ล้างการเลือก"),
        selection_select_all: Cow::Borrowed("เลือกทั้งหมด"),
        selection_count_a11y: Cow::Borrowed("เลือกแล้ว {count} รายการ"),
        split_button_show_menu_a11y: Cow::Borrowed("แสดงเมนู"),
        toolbar_more_options_a11y: Cow::Borrowed("ตัวเลือกเพิ่มเติม"),
        toolbar_action_fallback_a11y: Cow::Borrowed("การดำเนินการ {n}"),
        fab_menu_a11y: Cow::Borrowed("เมนู"),
        chip_remove_a11y: Cow::Borrowed("ลบ"),
        badge_notifications_a11y: Cow::Borrowed("การแจ้งเตือน: {count}"),
        badge_notifications_none_a11y: Cow::Borrowed("การแจ้งเตือน"),
        dropdown_hint: Cow::Borrowed("เลือก"),
        dropdown_empty: Cow::Borrowed("ไม่พบรายการ"),
        dropdown_search_hint: Cow::Borrowed("ค้นหา…"),
        date_picker: DatePickerStrings {
            weekday_initials: ["อา", "จ", "อ", "พ", "พฤ", "ศ", "ส"],
            first_day_of_week: 0,
            help_text: "เลือกวันที่",
            no_date_selected: "วันที่",
            cancel_label: "ยกเลิก",
            confirm_label: "ตกลง",
            field_label: "ป้อนวันที่",
            field_hint: "วว/ดด/ปปปป",
            error_format: "รูปแบบไม่ถูกต้อง",
            error_invalid: "อยู่นอกช่วง",
            previous_month: "เดือนก่อนหน้า",
            next_month: "เดือนถัดไป",
            select_year: "เลือกปี",
            input_mode: "สลับเป็นโหมดป้อนข้อความ",
            calendar_mode: "สลับเป็นโหมดปฏิทิน",
            ..base.date_picker
        },
        time_picker: TimePickerStringsCow {
            dial_help: Cow::Borrowed("เลือกเวลา"),
            input_help: Cow::Borrowed("ป้อนเวลา"),
            hour_label: Cow::Borrowed("ชั่วโมง"),
            minute_label: Cow::Borrowed("นาที"),
            invalid_time: Cow::Borrowed("ป้อนเวลาที่ถูกต้อง"),
            am: Cow::Borrowed("ก่อนเที่ยง"),
            pm: Cow::Borrowed("หลังเที่ยง"),
            cancel: Cow::Borrowed("ยกเลิก"),
            confirm: Cow::Borrowed("ตกลง"),
            dial_mode_tooltip: Cow::Borrowed("สลับเป็นโหมดหน้าปัด"),
            input_mode_tooltip: Cow::Borrowed("สลับเป็นโหมดป้อนข้อความ"),
            hour_mode_announcement: Cow::Borrowed("เลือกชั่วโมง"),
            minute_mode_announcement: Cow::Borrowed("เลือกนาที"),
        },
    }
}
