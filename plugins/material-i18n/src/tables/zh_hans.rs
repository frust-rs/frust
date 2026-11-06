//! Simplified Chinese table (machine-drafted; pending native review).

use std::borrow::Cow;

use frust_material::{DatePickerStrings, MaterialStrings, TimePickerStringsCow};

pub fn strings() -> MaterialStrings {
    let base = MaterialStrings::ENGLISH;
    MaterialStrings {
        dialog_cancel: Cow::Borrowed("取消"),
        dialog_confirm: Cow::Borrowed("确定"),
        dialog_close_a11y: Cow::Borrowed("关闭"),
        snackbar_dismiss_a11y: Cow::Borrowed("关闭"),
        rail_collapse_a11y: Cow::Borrowed("收起"),
        rail_expand_a11y: Cow::Borrowed("展开"),
        drawer_a11y: Cow::Borrowed("导航抽屉"),
        search_back_a11y: Cow::Borrowed("返回"),
        search_clear_a11y: Cow::Borrowed("清除"),
        selection_clear_a11y: Cow::Borrowed("取消选择"),
        selection_select_all: Cow::Borrowed("全选"),
        selection_count_a11y: Cow::Borrowed("已选择 {count} 项"),
        split_button_show_menu_a11y: Cow::Borrowed("显示菜单"),
        toolbar_more_options_a11y: Cow::Borrowed("更多选项"),
        toolbar_action_fallback_a11y: Cow::Borrowed("操作 {n}"),
        fab_menu_a11y: Cow::Borrowed("菜单"),
        chip_remove_a11y: Cow::Borrowed("删除"),
        badge_notifications_a11y: Cow::Borrowed("通知：{count}"),
        badge_notifications_none_a11y: Cow::Borrowed("通知"),
        dropdown_hint: Cow::Borrowed("选择"),
        dropdown_empty: Cow::Borrowed("未找到任何项"),
        dropdown_search_hint: Cow::Borrowed("搜索…"),
        date_picker: DatePickerStrings {
            weekday_initials: ["日", "一", "二", "三", "四", "五", "六"],
            first_day_of_week: 0,
            help_text: "选择日期",
            no_date_selected: "日期",
            cancel_label: "取消",
            confirm_label: "确定",
            field_label: "输入日期",
            field_hint: "yyyy/mm/dd",
            error_format: "格式无效。",
            error_invalid: "超出范围。",
            previous_month: "上个月",
            next_month: "下个月",
            select_year: "选择年份",
            input_mode: "切换到文本输入模式",
            calendar_mode: "切换到日历模式",
            ..base.date_picker
        },
        time_picker: TimePickerStringsCow {
            dial_help: Cow::Borrowed("选择时间"),
            input_help: Cow::Borrowed("输入时间"),
            hour_label: Cow::Borrowed("时"),
            minute_label: Cow::Borrowed("分"),
            invalid_time: Cow::Borrowed("请输入有效的时间"),
            am: Cow::Borrowed("上午"),
            pm: Cow::Borrowed("下午"),
            cancel: Cow::Borrowed("取消"),
            confirm: Cow::Borrowed("确定"),
            dial_mode_tooltip: Cow::Borrowed("切换到表盘模式"),
            input_mode_tooltip: Cow::Borrowed("切换到文本输入模式"),
            hour_mode_announcement: Cow::Borrowed("选择小时"),
            minute_mode_announcement: Cow::Borrowed("选择分钟"),
        },
    }
}
