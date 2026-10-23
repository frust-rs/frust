//! Japanese table (machine-drafted; pending native review).

use std::borrow::Cow;

use frust_material::{DatePickerStrings, MaterialStrings, TimePickerStringsCow};

pub fn strings() -> MaterialStrings {
    let base = MaterialStrings::ENGLISH;
    MaterialStrings {
        dialog_cancel: Cow::Borrowed("キャンセル"),
        dialog_confirm: Cow::Borrowed("OK"),
        dialog_close_a11y: Cow::Borrowed("閉じる"),
        snackbar_dismiss_a11y: Cow::Borrowed("閉じる"),
        rail_collapse_a11y: Cow::Borrowed("折りたたむ"),
        rail_expand_a11y: Cow::Borrowed("展開"),
        drawer_a11y: Cow::Borrowed("ナビゲーションドロワー"),
        search_back_a11y: Cow::Borrowed("戻る"),
        search_clear_a11y: Cow::Borrowed("クリア"),
        selection_clear_a11y: Cow::Borrowed("選択を解除"),
        selection_select_all: Cow::Borrowed("すべて選択"),
        selection_count_a11y: Cow::Borrowed("{count}件選択中"),
        split_button_show_menu_a11y: Cow::Borrowed("メニューを表示"),
        toolbar_more_options_a11y: Cow::Borrowed("その他のオプション"),
        toolbar_action_fallback_a11y: Cow::Borrowed("アクション {n}"),
        fab_menu_a11y: Cow::Borrowed("メニュー"),
        chip_remove_a11y: Cow::Borrowed("削除"),
        badge_notifications_a11y: Cow::Borrowed("通知: {count}"),
        badge_notifications_none_a11y: Cow::Borrowed("通知"),
        dropdown_hint: Cow::Borrowed("選択"),
        dropdown_empty: Cow::Borrowed("項目が見つかりません"),
        dropdown_search_hint: Cow::Borrowed("検索…"),
        date_picker: DatePickerStrings {
            weekday_initials: ["日", "月", "火", "水", "木", "金", "土"],
            first_day_of_week: 0,
            help_text: "日付を選択",
            no_date_selected: "日付",
            cancel_label: "キャンセル",
            confirm_label: "OK",
            field_label: "日付を入力",
            field_hint: "yyyy/mm/dd",
            error_format: "形式が正しくありません。",
            error_invalid: "範囲外です。",
            previous_month: "前の月",
            next_month: "次の月",
            select_year: "年を選択",
            input_mode: "テキスト入力モードに切り替え",
            calendar_mode: "カレンダーモードに切り替え",
            ..base.date_picker
        },
        time_picker: TimePickerStringsCow {
            dial_help: Cow::Borrowed("時刻を選択"),
            input_help: Cow::Borrowed("時刻を入力"),
            hour_label: Cow::Borrowed("時"),
            minute_label: Cow::Borrowed("分"),
            invalid_time: Cow::Borrowed("有効な時刻を入力してください"),
            am: Cow::Borrowed("午前"),
            pm: Cow::Borrowed("午後"),
            cancel: Cow::Borrowed("キャンセル"),
            confirm: Cow::Borrowed("OK"),
            dial_mode_tooltip: Cow::Borrowed("ダイヤルモードに切り替え"),
            input_mode_tooltip: Cow::Borrowed("テキスト入力モードに切り替え"),
            hour_mode_announcement: Cow::Borrowed("時を選択"),
            minute_mode_announcement: Cow::Borrowed("分を選択"),
        },
    }
}
