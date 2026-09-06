//! Vendored Material Design Icons — GENERATED, do not hand-edit.
//!
//! The 88 distinct glyphs `frust_material`'s catalog and the
//! `material_3_expressive` reference library's example app actually use,
//! each an [`IconSource`](frust::IconSource) carrying an upstream
//! `google/material-design-icons` classic-family path as SVG `d` data in a
//! 24×24 design box. Pass one to [`frust::icon`] (or convert it via
//! [`frust::IconData::from`]) to paint it — access is namespaced
//! (`frust_material::icons::CHECK`), not flat re-exported at the crate root.
//!
//! # Provenance & license
//!
//! `google/material-design-icons` is licensed Apache-2.0 (Google); the path
//! data below is vendored verbatim (coordinates already authored against a
//! `viewBox="0 0 24 24"` box needing no rescale — see
//! `plugins/material/scripts/gen_icons.py`'s module docstring), minus each
//! source SVG's invisible `fill="none"` click-target keyline path.
//!
//! # Regenerating
//!
//! ```text
//! python3 plugins/material/scripts/gen_icons.py
//! cargo fmt -p frust-material
//! ```
//!
//! See `plugins/material/scripts/gen_icons.py` for the fetch/normalize
//! pipeline and its own `ICON_SET`/`BASE_CATEGORY` source-of-truth tables;
//! the trailing `cargo fmt` collapses this generator's raw double blank line
//! before the first entry down to the single blank line this file carries,
//! the same reason the baseline `scripts/gen_icons.py` documents.

use frust::IconSource;

/// The design-box side length every entry below is authored against.
const D: f64 = 24.0;

/// `account_circle`
pub const ACCOUNT_CIRCLE: IconSource = IconSource {
    d: "M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm0 4c1.93 0 3.5 1.57 3.5 3.5S13.93 13 12 13s-3.5-1.57-3.5-3.5S10.07 6 12 6zm0 14c-2.03 0-4.43-.82-6.14-2.88C7.55 15.8 9.68 15 12 15s4.45.8 6.14 2.12C16.43 19.18 14.03 20 12 20z",
    design: D,
};

/// `add`
pub const ADD: IconSource = IconSource {
    d: "M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6v2z",
    design: D,
};

/// `apps`
pub const APPS: IconSource = IconSource {
    d: "M4 8h4V4H4v4zm6 12h4v-4h-4v4zm-6 0h4v-4H4v4zm0-6h4v-4H4v4zm6 0h4v-4h-4v4zm6-10v4h4V4h-4zm-6 4h4V4h-4v4zm6 6h4v-4h-4v4zm0 6h4v-4h-4v4z",
    design: D,
};

/// `archive`
pub const ARCHIVE: IconSource = IconSource {
    d: "M20.54 5.23l-1.39-1.68C18.88 3.21 18.47 3 18 3H6c-.47 0-.88.21-1.16.55L3.46 5.23C3.17 5.57 3 6.02 3 6.5V19c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V6.5c0-.48-.17-.93-.46-1.27zM12 17.5L6.5 12H10v-2h4v2h3.5L12 17.5zM5.12 5l.81-1h12l.94 1H5.12z",
    design: D,
};

/// `arrow_back`
pub const ARROW_BACK: IconSource = IconSource {
    d: "M20 11H7.83l5.59-5.59L12 4l-8 8 8 8 1.41-1.41L7.83 13H20v-2z",
    design: D,
};

/// `arrow_back_rounded`
pub const ARROW_BACK_ROUNDED: IconSource = IconSource {
    d: "M19 11H7.83l4.88-4.88c.39-.39.39-1.03 0-1.42-.39-.39-1.02-.39-1.41 0l-6.59 6.59c-.39.39-.39 1.02 0 1.41l6.59 6.59c.39.39 1.02.39 1.41 0 .39-.39.39-1.02 0-1.41L7.83 13H19c.55 0 1-.45 1-1s-.45-1-1-1z",
    design: D,
};

/// `arrow_drop_down`
pub const ARROW_DROP_DOWN: IconSource = IconSource {
    d: "M7 10l5 5 5-5z",
    design: D,
};

/// `arrow_drop_down_circle`
pub const ARROW_DROP_DOWN_CIRCLE: IconSource = IconSource {
    d: "M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm0 12l-4-4h8l-4 4z",
    design: D,
};

/// `arrow_drop_up`
pub const ARROW_DROP_UP: IconSource = IconSource {
    d: "M7 14l5-5 5 5z",
    design: D,
};

/// `arrow_right_rounded`
pub const ARROW_RIGHT_ROUNDED: IconSource = IconSource {
    d: "M11.71 15.29l2.59-2.59c.39-.39.39-1.02 0-1.41L11.71 8.7c-.63-.62-1.71-.18-1.71.71v5.17c0 .9 1.08 1.34 1.71.71z",
    design: D,
};

/// `autorenew`
pub const AUTORENEW: IconSource = IconSource {
    d: "M12 6v3l4-4-4-4v3c-4.42 0-8 3.58-8 8 0 1.57.46 3.03 1.24 4.26L6.7 14.8c-.45-.83-.7-1.79-.7-2.8 0-3.31 2.69-6 6-6zm6.76 1.74L17.3 9.2c.44.84.7 1.79.7 2.8 0 3.31-2.69 6-6 6v-3l-4 4 4 4v-3c4.42 0 8-3.58 8-8 0-1.57-.46-3.03-1.24-4.26z",
    design: D,
};

/// `battery_alert`
pub const BATTERY_ALERT: IconSource = IconSource {
    d: "M15.67 4H14V2h-4v2H8.33C7.6 4 7 4.6 7 5.33v15.33C7 21.4 7.6 22 8.33 22h7.33c.74 0 1.34-.6 1.34-1.33V5.33C17 4.6 16.4 4 15.67 4zM13 18h-2v-2h2v2zm0-4h-2V9h2v5z",
    design: D,
};

/// `bookmark`
pub const BOOKMARK: IconSource = IconSource {
    d: "M17 3H7c-1.1 0-1.99.9-1.99 2L5 21l7-3 7 3V5c0-1.1-.9-2-2-2z",
    design: D,
};

/// `build`
pub const BUILD: IconSource = IconSource {
    d: "M22.7 19l-9.1-9.1c.9-2.3.4-5-1.5-6.9-2-2-5-2.4-7.4-1.3L9 6 6 9 1.6 4.7C.4 7.1.9 10.1 2.9 12.1c1.9 1.9 4.6 2.4 6.9 1.5l9.1 9.1c.4.4 1 .4 1.4 0l2.3-2.3c.5-.4.5-1.1.1-1.4z",
    design: D,
};

/// `calendar_month`
pub const CALENDAR_MONTH: IconSource = IconSource {
    d: "M19,4h-1V2h-2v2H8V2H6v2H5C3.89,4,3.01,4.9,3.01,6L3,20c0,1.1,0.89,2,2,2h14c1.1,0,2-0.9,2-2V6C21,4.9,20.1,4,19,4z M19,20 H5V10h14V20z M9,14H7v-2h2V14z M13,14h-2v-2h2V14z M17,14h-2v-2h2V14z M9,18H7v-2h2V18z M13,18h-2v-2h2V18z M17,18h-2v-2h2V18z",
    design: D,
};

/// `calendar_today`
pub const CALENDAR_TODAY: IconSource = IconSource {
    d: "M20 3h-1V1h-2v2H7V1H5v2H4c-1.1 0-2 .9-2 2v16c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 18H4V8h16v13z",
    design: D,
};

/// `calendar_view_week`
pub const CALENDAR_VIEW_WEEK: IconSource = IconSource {
    d: "M20,4H4C2.9,4,2,4.9,2,6v12c0,1.1,0.9,2,2,2h16c1.1,0,2-0.9,2-2V6C22,4.9,21.1,4,20,4z M13,6h2.5v12H13V6z M11,18H8.5V6H11 V18z M4,6h2.5v12H4V6z M20,18h-2.5V6H20V18z",
    design: D,
};

/// `category`
pub const CATEGORY: IconSource = IconSource {
    d: "M12 2l-5.5 9h11z M3 13.5h8v8H3z",
    design: D,
};

/// `chat_bubble`
pub const CHAT_BUBBLE: IconSource = IconSource {
    d: "M20 2H4c-1.1 0-2 .9-2 2v18l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2z",
    design: D,
};

/// `check`
pub const CHECK: IconSource = IconSource {
    d: "M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z",
    design: D,
};

/// `check_box`
pub const CHECK_BOX: IconSource = IconSource {
    d: "M19 3H5c-1.11 0-2 .9-2 2v14c0 1.1.89 2 2 2h14c1.11 0 2-.9 2-2V5c0-1.1-.89-2-2-2zm-9 14l-5-5 1.41-1.41L10 14.17l7.59-7.59L19 8l-9 9z",
    design: D,
};

/// `check_box_outline_blank_rounded`
pub const CHECK_BOX_OUTLINE_BLANK_ROUNDED: IconSource = IconSource {
    d: "M18 19H6c-.55 0-1-.45-1-1V6c0-.55.45-1 1-1h12c.55 0 1 .45 1 1v12c0 .55-.45 1-1 1zm1-16H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2z",
    design: D,
};

/// `check_box_rounded`
pub const CHECK_BOX_ROUNDED: IconSource = IconSource {
    d: "M19 3H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm-8.29 13.29c-.39.39-1.02.39-1.41 0L5.71 12.7c-.39-.39-.39-1.02 0-1.41.39-.39 1.02-.39 1.41 0L10 14.17l6.88-6.88c.39-.39 1.02-.39 1.41 0 .39.39.39 1.02 0 1.41l-7.58 7.59z",
    design: D,
};

/// `check_rounded`
pub const CHECK_ROUNDED: IconSource = IconSource {
    d: "M9 16.17L5.53 12.7c-.39-.39-1.02-.39-1.41 0-.39.39-.39 1.02 0 1.41l4.18 4.18c.39.39 1.02.39 1.41 0L20.29 7.71c.39-.39.39-1.02 0-1.41-.39-.39-1.02-.39-1.41 0L9 16.17z",
    design: D,
};

/// `chevron_left`
pub const CHEVRON_LEFT: IconSource = IconSource {
    d: "M15.41 7.41L14 6l-6 6 6 6 1.41-1.41L10.83 12z",
    design: D,
};

/// `chevron_right`
pub const CHEVRON_RIGHT: IconSource = IconSource {
    d: "M10 6L8.59 7.41 13.17 12l-4.58 4.59L10 18l6-6z",
    design: D,
};

/// `close`
pub const CLOSE: IconSource = IconSource {
    d: "M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z",
    design: D,
};

/// `content_copy`
pub const CONTENT_COPY: IconSource = IconSource {
    d: "M16 1H4c-1.1 0-2 .9-2 2v14h2V3h12V1zm3 4H8c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h11c1.1 0 2-.9 2-2V7c0-1.1-.9-2-2-2zm0 16H8V7h11v14z",
    design: D,
};

/// `crop_square`
pub const CROP_SQUARE: IconSource = IconSource {
    d: "M18 4H6c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h12c1.1 0 2-.9 2-2V6c0-1.1-.9-2-2-2zm0 14H6V6h12v12z",
    design: D,
};

/// `dark_mode`
pub const DARK_MODE: IconSource = IconSource {
    d: "M12,3c-4.97,0-9,4.03-9,9s4.03,9,9,9s9-4.03,9-9c0-0.46-0.04-0.92-0.1-1.36c-0.98,1.37-2.58,2.26-4.4,2.26 c-2.98,0-5.4-2.42-5.4-5.4c0-1.81,0.89-3.42,2.26-4.4C12.92,3.04,12.46,3,12,3L12,3z",
    design: D,
};

/// `delete`
pub const DELETE: IconSource = IconSource {
    d: "M6 19c0 1.1.9 2 2 2h8c1.1 0 2-.9 2-2V7H6v12zM19 4h-3.5l-1-1h-5l-1 1H5v2h14V4z",
    design: D,
};

/// `edit`
pub const EDIT: IconSource = IconSource {
    d: "M3 17.25V21h3.75L17.81 9.94l-3.75-3.75L3 17.25zM20.71 7.04c.39-.39.39-1.02 0-1.41l-2.34-2.34c-.39-.39-1.02-.39-1.41 0l-1.83 1.83 3.75 3.75 1.83-1.83z",
    design: D,
};

/// `edit_outlined`
pub const EDIT_OUTLINED: IconSource = IconSource {
    d: "M14.06 9.02l.92.92L5.92 19H5v-.92l9.06-9.06M17.66 3c-.25 0-.51.1-.7.29l-1.83 1.83 3.75 3.75 1.83-1.83c.39-.39.39-1.02 0-1.41l-2.34-2.34c-.2-.2-.45-.29-.71-.29zm-3.6 3.19L3 17.25V21h3.75L17.81 9.94l-3.75-3.75z",
    design: D,
};

/// `error`
pub const ERROR: IconSource = IconSource {
    d: "M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-2h2v2zm0-4h-2V7h2v6z",
    design: D,
};

/// `expand_more_rounded`
pub const EXPAND_MORE_ROUNDED: IconSource = IconSource {
    d: "M15.88 9.29L12 13.17 8.12 9.29c-.39-.39-1.02-.39-1.41 0-.39.39-.39 1.02 0 1.41l4.59 4.59c.39.39 1.02.39 1.41 0l4.59-4.59c.39-.39.39-1.02 0-1.41-.39-.38-1.03-.39-1.42 0z",
    design: D,
};

/// `favorite`
pub const FAVORITE: IconSource = IconSource {
    d: "M12 21.35l-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54L12 21.35z",
    design: D,
};

/// `favorite_border`
pub const FAVORITE_BORDER: IconSource = IconSource {
    d: "M16.5 3c-1.74 0-3.41.81-4.5 2.09C10.91 3.81 9.24 3 7.5 3 4.42 3 2 5.42 2 8.5c0 3.78 3.4 6.86 8.55 11.54L12 21.35l1.45-1.32C18.6 15.36 22 12.28 22 8.5 22 5.42 19.58 3 16.5 3zm-4.4 15.55l-.1.1-.1-.1C7.14 14.24 4 11.39 4 8.5 4 6.5 5.5 5 7.5 5c1.54 0 3.04.99 3.57 2.36h1.87C13.46 5.99 14.96 5 16.5 5c2 0 3.5 1.5 3.5 3.5 0 2.89-3.14 5.74-7.9 10.05z",
    design: D,
};

/// `format_align_center`
pub const FORMAT_ALIGN_CENTER: IconSource = IconSource {
    d: "M7 15v2h10v-2H7zm-4 6h18v-2H3v2zm0-8h18v-2H3v2zm4-6v2h10V7H7zM3 3v2h18V3H3z",
    design: D,
};

/// `format_align_left`
pub const FORMAT_ALIGN_LEFT: IconSource = IconSource {
    d: "M15 15H3v2h12v-2zm0-8H3v2h12V7zM3 13h18v-2H3v2zm0 8h18v-2H3v2zM3 3v2h18V3H3z",
    design: D,
};

/// `format_align_right`
pub const FORMAT_ALIGN_RIGHT: IconSource = IconSource {
    d: "M3 21h18v-2H3v2zm6-4h12v-2H9v2zm-6-4h18v-2H3v2zm6-4h12V7H9v2zM3 3v2h18V3H3z",
    design: D,
};

/// `home`
pub const HOME: IconSource = IconSource {
    d: "M10 20v-6h4v6h5v-8h3L12 3 2 12h3v8z",
    design: D,
};

/// `horizontal_rule`
pub const HORIZONTAL_RULE: IconSource = IconSource {
    d: "M 4 11 H 20 V 13 H 4 Z",
    design: D,
};

/// `hourglass_empty`
pub const HOURGLASS_EMPTY: IconSource = IconSource {
    d: "M6 2v6h.01L6 8.01 10 12l-4 4 .01.01H6V22h12v-5.99h-.01L18 16l-4-4 4-3.99-.01-.01H18V2H6zm10 14.5V20H8v-3.5l4-4 4 4zm-4-5l-4-4V4h8v3.5l-4 4z",
    design: D,
};

/// `image`
pub const IMAGE: IconSource = IconSource {
    d: "M21 19V5c0-1.1-.9-2-2-2H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2zM8.5 13.5l2.5 3.01L14.5 12l4.5 6H5l3.5-4.5z",
    design: D,
};

/// `inbox`
pub const INBOX: IconSource = IconSource {
    d: "M19 3H4.99c-1.11 0-1.98.89-1.98 2L3 19c0 1.1.88 2 1.99 2H19c1.1 0 2-.9 2-2V5c0-1.11-.9-2-2-2zm0 12h-4c0 1.66-1.35 3-3 3s-3-1.34-3-3H4.99V5H19v10z",
    design: D,
};

/// `info`
pub const INFO: IconSource = IconSource {
    d: "M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-6h2v6zm0-8h-2V7h2v2z",
    design: D,
};

/// `keyboard_arrow_down`
pub const KEYBOARD_ARROW_DOWN: IconSource = IconSource {
    d: "M7.41 8.59L12 13.17l4.59-4.58L18 10l-6 6-6-6 1.41-1.41z",
    design: D,
};

/// `label`
pub const LABEL: IconSource = IconSource {
    d: "M17.63 5.84C17.27 5.33 16.67 5 16 5L5 5.01C3.9 5.01 3 5.9 3 7v10c0 1.1.9 1.99 2 1.99L16 19c.67 0 1.27-.33 1.63-.84L22 12l-4.37-6.16z",
    design: D,
};

/// `light_mode`
pub const LIGHT_MODE: IconSource = IconSource {
    d: "M12,7c-2.76,0-5,2.24-5,5s2.24,5,5,5s5-2.24,5-5S14.76,7,12,7L12,7z M2,13l2,0c0.55,0,1-0.45,1-1s-0.45-1-1-1l-2,0 c-0.55,0-1,0.45-1,1S1.45,13,2,13z M20,13l2,0c0.55,0,1-0.45,1-1s-0.45-1-1-1l-2,0c-0.55,0-1,0.45-1,1S19.45,13,20,13z M11,2v2 c0,0.55,0.45,1,1,1s1-0.45,1-1V2c0-0.55-0.45-1-1-1S11,1.45,11,2z M11,20v2c0,0.55,0.45,1,1,1s1-0.45,1-1v-2c0-0.55-0.45-1-1-1 C11.45,19,11,19.45,11,20z M5.99,4.58c-0.39-0.39-1.03-0.39-1.41,0c-0.39,0.39-0.39,1.03,0,1.41l1.06,1.06 c0.39,0.39,1.03,0.39,1.41,0s0.39-1.03,0-1.41L5.99,4.58z M18.36,16.95c-0.39-0.39-1.03-0.39-1.41,0c-0.39,0.39-0.39,1.03,0,1.41 l1.06,1.06c0.39,0.39,1.03,0.39,1.41,0c0.39-0.39,0.39-1.03,0-1.41L18.36,16.95z M19.42,5.99c0.39-0.39,0.39-1.03,0-1.41 c-0.39-0.39-1.03-0.39-1.41,0l-1.06,1.06c-0.39,0.39-0.39,1.03,0,1.41s1.03,0.39,1.41,0L19.42,5.99z M7.05,18.36 c0.39-0.39,0.39-1.03,0-1.41c-0.39-0.39-1.03-0.39-1.41,0l-1.06,1.06c-0.39,0.39-0.39,1.03,0,1.41s1.03,0.39,1.41,0L7.05,18.36z",
    design: D,
};

/// `linear_scale`
pub const LINEAR_SCALE: IconSource = IconSource {
    d: "M19.5,9.5c-1.03,0-1.9,0.62-2.29,1.5h-2.92C13.9,10.12,13.03,9.5,12,9.5s-1.9,0.62-2.29,1.5H6.79 C6.4,10.12,5.53,9.5,4.5,9.5C3.12,9.5,2,10.62,2,12s1.12,2.5,2.5,2.5c1.03,0,1.9-0.62,2.29-1.5h2.92c0.39,0.88,1.26,1.5,2.29,1.5 s1.9-0.62,2.29-1.5h2.92c0.39,0.88,1.26,1.5,2.29,1.5c1.38,0,2.5-1.12,2.5-2.5S20.88,9.5,19.5,9.5z",
    design: D,
};

/// `list`
pub const LIST: IconSource = IconSource {
    d: "M3 13h2v-2H3v2zm0 4h2v-2H3v2zm0-8h2V7H3v2zm4 4h14v-2H7v2zm0 4h14v-2H7v2zM7 7v2h14V7H7z",
    design: D,
};

/// `mail`
pub const MAIL: IconSource = IconSource {
    d: "M20 4H4c-1.1 0-1.99.9-1.99 2L2 18c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V6c0-1.1-.9-2-2-2zm0 4l-8 5-8-5V6l8 5 8-5v2z",
    design: D,
};

/// `menu`
pub const MENU: IconSource = IconSource {
    d: "M3 18h18v-2H3v2zm0-5h18v-2H3v2zm0-7v2h18V6H3z",
    design: D,
};

/// `menu_open`
pub const MENU_OPEN: IconSource = IconSource {
    d: "M3 18h13v-2H3v2zm0-5h10v-2H3v2zm0-7v2h13V6H3zm18 9.59L17.42 12 21 8.41 19.59 7l-5 5 5 5L21 15.59z",
    design: D,
};

/// `message`
pub const MESSAGE: IconSource = IconSource {
    d: "M20 2H4c-1.1 0-1.99.9-1.99 2L2 22l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm-2 12H6v-2h12v2zm0-3H6V9h12v2zm0-3H6V6h12v2z",
    design: D,
};

/// `mic`
pub const MIC: IconSource = IconSource {
    d: "M12 14c1.66 0 2.99-1.34 2.99-3L15 5c0-1.66-1.34-3-3-3S9 3.34 9 5v6c0 1.66 1.34 3 3 3zm5.3-3c0 3-2.54 5.1-5.3 5.1S6.7 14 6.7 11H5c0 3.41 2.72 6.23 6 6.72V21h2v-3.28c3.28-.48 6-3.3 6-6.72h-1.7z",
    design: D,
};

/// `more_horiz`
pub const MORE_HORIZ: IconSource = IconSource {
    d: "M6 10c-1.1 0-2 .9-2 2s.9 2 2 2 2-.9 2-2-.9-2-2-2zm12 0c-1.1 0-2 .9-2 2s.9 2 2 2 2-.9 2-2-.9-2-2-2zm-6 0c-1.1 0-2 .9-2 2s.9 2 2 2 2-.9 2-2-.9-2-2-2z",
    design: D,
};

/// `more_vert`
pub const MORE_VERT: IconSource = IconSource {
    d: "M12 8c1.1 0 2-.9 2-2s-.9-2-2-2-2 .9-2 2 .9 2 2 2zm0 2c-1.1 0-2 .9-2 2s.9 2 2 2 2-.9 2-2-.9-2-2-2zm0 6c-1.1 0-2 .9-2 2s.9 2 2 2 2-.9 2-2-.9-2-2-2z",
    design: D,
};

/// `notifications`
pub const NOTIFICATIONS: IconSource = IconSource {
    d: "M12 22c1.1 0 2-.9 2-2h-4c0 1.1.89 2 2 2zm6-6v-5c0-3.07-1.64-5.64-4.5-6.32V4c0-.83-.67-1.5-1.5-1.5s-1.5.67-1.5 1.5v.68C7.63 5.36 6 7.92 6 11v5l-2 2v1h16v-1l-2-2z",
    design: D,
};

/// `palette`
pub const PALETTE: IconSource = IconSource {
    d: "M12,2C6.49,2,2,6.49,2,12s4.49,10,10,10c1.38,0,2.5-1.12,2.5-2.5c0-0.61-0.23-1.2-0.64-1.67c-0.08-0.1-0.13-0.21-0.13-0.33 c0-0.28,0.22-0.5,0.5-0.5H16c3.31,0,6-2.69,6-6C22,6.04,17.51,2,12,2z M17.5,13c-0.83,0-1.5-0.67-1.5-1.5c0-0.83,0.67-1.5,1.5-1.5 s1.5,0.67,1.5,1.5C19,12.33,18.33,13,17.5,13z M14.5,9C13.67,9,13,8.33,13,7.5C13,6.67,13.67,6,14.5,6S16,6.67,16,7.5 C16,8.33,15.33,9,14.5,9z M5,11.5C5,10.67,5.67,10,6.5,10S8,10.67,8,11.5C8,12.33,7.33,13,6.5,13S5,12.33,5,11.5z M11,7.5 C11,8.33,10.33,9,9.5,9S8,8.33,8,7.5C8,6.67,8.67,6,9.5,6S11,6.67,11,7.5z",
    design: D,
};

/// `radio_button_checked`
pub const RADIO_BUTTON_CHECKED: IconSource = IconSource {
    d: "M12 7c-2.76 0-5 2.24-5 5s2.24 5 5 5 5-2.24 5-5-2.24-5-5-5zm0-5C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm0 18c-4.42 0-8-3.58-8-8s3.58-8 8-8 8 3.58 8 8-3.58 8-8 8z",
    design: D,
};

/// `refresh`
pub const REFRESH: IconSource = IconSource {
    d: "M17.65 6.35C16.2 4.9 14.21 4 12 4c-4.42 0-7.99 3.58-7.99 8s3.57 8 7.99 8c3.73 0 6.84-2.55 7.73-6h-2.08c-.82 2.33-3.04 4-5.65 4-3.31 0-6-2.69-6-6s2.69-6 6-6c1.66 0 3.14.69 4.22 1.78L13 11h7V4l-2.35 2.35z",
    design: D,
};

/// `report`
pub const REPORT: IconSource = IconSource {
    d: "M15.73 3H8.27L3 8.27v7.46L8.27 21h7.46L21 15.73V8.27L15.73 3zM12 17.3c-.72 0-1.3-.58-1.3-1.3 0-.72.58-1.3 1.3-1.3.72 0 1.3.58 1.3 1.3 0 .72-.58 1.3-1.3 1.3zm1-4.3h-2V7h2v6z",
    design: D,
};

/// `save`
pub const SAVE: IconSource = IconSource {
    d: "M17 3H5c-1.11 0-2 .9-2 2v14c0 1.1.89 2 2 2h14c1.1 0 2-.9 2-2V7l-4-4zm-5 16c-1.66 0-3-1.34-3-3s1.34-3 3-3 3 1.34 3 3-1.34 3-3 3zm3-10H5V5h10v4z",
    design: D,
};

/// `schedule`
pub const SCHEDULE: IconSource = IconSource {
    d: "M11.99 2C6.47 2 2 6.48 2 12s4.47 10 9.99 10C17.52 22 22 17.52 22 12S17.52 2 11.99 2zM12 20c-4.42 0-8-3.58-8-8s3.58-8 8-8 8 3.58 8 8-3.58 8-8 8z M12.5 7H11v6l5.25 3.15.75-1.23-4.5-2.67z",
    design: D,
};

/// `search`
pub const SEARCH: IconSource = IconSource {
    d: "M15.5 14h-.79l-.28-.27C15.41 12.59 16 11.11 16 9.5 16 5.91 13.09 3 9.5 3S3 5.91 3 9.5 5.91 16 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z",
    design: D,
};

/// `select_all`
pub const SELECT_ALL: IconSource = IconSource {
    d: "M3 5h2V3c-1.1 0-2 .9-2 2zm0 8h2v-2H3v2zm4 8h2v-2H7v2zM3 9h2V7H3v2zm10-6h-2v2h2V3zm6 0v2h2c0-1.1-.9-2-2-2zM5 21v-2H3c0 1.1.9 2 2 2zm-2-4h2v-2H3v2zM9 3H7v2h2V3zm2 18h2v-2h-2v2zm8-8h2v-2h-2v2zm0 8c1.1 0 2-.9 2-2h-2v2zm0-12h2V7h-2v2zm0 8h2v-2h-2v2zm-4 4h2v-2h-2v2zm0-16h2V3h-2v2zM7 17h10V7H7v10zm2-8h6v6H9V9z",
    design: D,
};

/// `send`
pub const SEND: IconSource = IconSource {
    d: "M2.01 21L23 12 2.01 3 2 10l15 2-15 2z",
    design: D,
};

/// `share`
pub const SHARE: IconSource = IconSource {
    d: "M18 16.08c-.76 0-1.44.3-1.96.77L8.91 12.7c.05-.23.09-.46.09-.7s-.04-.47-.09-.7l7.05-4.11c.54.5 1.25.81 2.04.81 1.66 0 3-1.34 3-3s-1.34-3-3-3-3 1.34-3 3c0 .24.04.47.09.7L8.04 9.81C7.5 9.31 6.79 9 6 9c-1.66 0-3 1.34-3 3s1.34 3 3 3c.79 0 1.5-.31 2.04-.81l7.12 4.16c-.05.21-.08.43-.08.65 0 1.61 1.31 2.92 2.92 2.92 1.61 0 2.92-1.31 2.92-2.92s-1.31-2.92-2.92-2.92z",
    design: D,
};

/// `smart_button`
pub const SMART_BUTTON: IconSource = IconSource {
    d: "M22,9v6c0,1.1-0.9,2-2,2h-1l0-2h1V9H4v6h6v2H4c-1.1,0-2-0.9-2-2V9c0-1.1,0.9-2,2-2h16C21.1,7,22,7.9,22,9z M14.5,19 l1.09-2.41L18,15.5l-2.41-1.09L14.5,12l-1.09,2.41L11,15.5l2.41,1.09L14.5,19z M17,14l0.62-1.38L19,12l-1.38-0.62L17,10l-0.62,1.38 L15,12l1.38,0.62L17,14z M14.5,19l1.09-2.41L18,15.5l-2.41-1.09L14.5,12l-1.09,2.41L11,15.5l2.41,1.09L14.5,19z M17,14l0.62-1.38 L19,12l-1.38-0.62L17,10l-0.62,1.38L15,12l1.38,0.62L17,14z",
    design: D,
};

/// `space_dashboard`
pub const SPACE_DASHBOARD: IconSource = IconSource {
    d: "M11,21H5c-1.1,0-2-0.9-2-2V5c0-1.1,0.9-2,2-2h6V21z M13,21h6c1.1,0,2-0.9,2-2v-7h-8V21z M21,10V5c0-1.1-0.9-2-2-2h-6v7H21z",
    design: D,
};

/// `star`
pub const STAR: IconSource = IconSource {
    d: "M12 17.27 18.18 21l-1.64-7.03L22 9.24l-7.19-.61L12 2 9.19 8.63 2 9.24l5.46 4.73L5.82 21z",
    design: D,
};

/// `star_outline`
pub const STAR_OUTLINE: IconSource = IconSource {
    d: "M22 9.24l-7.19-.62L12 2 9.19 8.63 2 9.24l5.46 4.73L5.82 21 12 17.27 18.18 21l-1.63-7.03L22 9.24zM12 15.4l-3.76 2.27 1-4.28-3.32-2.88 4.38-.38L12 6.1l1.71 4.04 4.38.38-3.32 2.88 1 4.28L12 15.4z",
    design: D,
};

/// `system_update`
pub const SYSTEM_UPDATE: IconSource = IconSource {
    d: "M17 1.01L7 1c-1.1 0-2 .9-2 2v18c0 1.1.9 2 2 2h10c1.1 0 2-.9 2-2V3c0-1.1-.9-1.99-2-1.99zM17 19H7V5h10v14zm-1-6h-3V8h-2v5H8l4 4 4-4z",
    design: D,
};

/// `tab`
pub const TAB: IconSource = IconSource {
    d: "M21 3H3c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h18c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 16H3V5h10v4h8v10z",
    design: D,
};

/// `tag`
pub const TAG: IconSource = IconSource {
    d: "M20 10V8h-4V4h-2v4h-4V4H8v4H4v2h4v4H4v2h4v4h2v-4h4v4h2v-4h4v-2h-4v-4h4zm-6 4h-4v-4h4v4z",
    design: D,
};

/// `text_fields`
pub const TEXT_FIELDS: IconSource = IconSource {
    d: "M2.5,4v3h5v12h3V7h5V4H2.5z M21.5,9h-9v3h3v7h3v-7h3V9z",
    design: D,
};

/// `toggle_on`
pub const TOGGLE_ON: IconSource = IconSource {
    d: "M17 7H7c-2.76 0-5 2.24-5 5s2.24 5 5 5h10c2.76 0 5-2.24 5-5s-2.24-5-5-5zm0 8c-1.66 0-3-1.34-3-3s1.34-3 3-3 3 1.34 3 3-1.34 3-3 3z",
    design: D,
};

/// `tune`
pub const TUNE: IconSource = IconSource {
    d: "M3 17v2h6v-2H3zM3 5v2h10V5H3zm10 16v-2h8v-2h-8v-2h-2v6h2zM7 9v2H3v2h4v2h2V9H7zm14 4v-2H11v2h10zm-6-4h2V7h4V5h-4V3h-2v6z",
    design: D,
};

/// `vertical_align_bottom`
pub const VERTICAL_ALIGN_BOTTOM: IconSource = IconSource {
    d: "M16 13h-3V3h-2v10H8l4 4 4-4zM4 19v2h16v-2H4z",
    design: D,
};

/// `vertical_split`
pub const VERTICAL_SPLIT: IconSource = IconSource {
    d: "M3 15h8v-2H3v2zm0 4h8v-2H3v2zm0-8h8V9H3v2zm0-6v2h8V5H3zm10 0h8v14h-8V5z",
    design: D,
};

/// `videocam`
pub const VIDEOCAM: IconSource = IconSource {
    d: "M17 10.5V7c0-.55-.45-1-1-1H4c-.55 0-1 .45-1 1v10c0 .55.45 1 1 1h12c.55 0 1-.45 1-1v-3.5l4 4v-11l-4 4z",
    design: D,
};

/// `view_carousel`
pub const VIEW_CAROUSEL: IconSource = IconSource {
    d: "M2,7h4v10H2V7z M7,19h10V5H7V19z M18,7h4v10h-4V7z",
    design: D,
};

/// `view_column`
pub const VIEW_COLUMN: IconSource = IconSource {
    d: "M14.67,5v14H9.33V5H14.67z M15.67,19H21V5h-5.33V19z M8.33,19V5H3v14H8.33z",
    design: D,
};

/// `view_sidebar`
pub const VIEW_SIDEBAR: IconSource = IconSource {
    d: "M16,20H2V4h14V20z M18,8h4V4h-4V8z M18,20h4v-4h-4V20z M18,14h4v-4h-4V14z",
    design: D,
};

/// `view_week`
pub const VIEW_WEEK: IconSource = IconSource {
    d: "M5.33,20H4c-1.1,0-2-0.9-2-2V6c0-1.1,0.9-2,2-2h1.33c1.1,0,2,0.9,2,2v12C7.33,19.1,6.44,20,5.33,20z M22,18V6 c0-1.1-0.9-2-2-2h-1.33c-1.1,0-2,0.9-2,2v12c0,1.1,0.9,2,2,2H20C21.11,20,22,19.1,22,18z M14.67,18V6c0-1.1-0.9-2-2-2h-1.33 c-1.1,0-2,0.9-2,2v12c0,1.1,0.9,2,2,2h1.33C13.77,20,14.67,19.1,14.67,18z",
    design: D,
};

/// `volume_up`
pub const VOLUME_UP: IconSource = IconSource {
    d: "M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z",
    design: D,
};

/// `web_asset`
pub const WEB_ASSET: IconSource = IconSource {
    d: "M19 4H5c-1.11 0-2 .9-2 2v12c0 1.1.89 2 2 2h14c1.1 0 2-.9 2-2V6c0-1.1-.89-2-2-2zm0 14H5V8h14v10z",
    design: D,
};

/// Every generated [`IconSource`] in this module, for exhaustive
/// iteration (e.g. a parse-validation test, or a picker gallery).
pub const ALL: &[IconSource] = &[
    ACCOUNT_CIRCLE,
    ADD,
    APPS,
    ARCHIVE,
    ARROW_BACK,
    ARROW_BACK_ROUNDED,
    ARROW_DROP_DOWN,
    ARROW_DROP_DOWN_CIRCLE,
    ARROW_DROP_UP,
    ARROW_RIGHT_ROUNDED,
    AUTORENEW,
    BATTERY_ALERT,
    BOOKMARK,
    BUILD,
    CALENDAR_MONTH,
    CALENDAR_TODAY,
    CALENDAR_VIEW_WEEK,
    CATEGORY,
    CHAT_BUBBLE,
    CHECK,
    CHECK_BOX,
    CHECK_BOX_OUTLINE_BLANK_ROUNDED,
    CHECK_BOX_ROUNDED,
    CHECK_ROUNDED,
    CHEVRON_LEFT,
    CHEVRON_RIGHT,
    CLOSE,
    CONTENT_COPY,
    CROP_SQUARE,
    DARK_MODE,
    DELETE,
    EDIT,
    EDIT_OUTLINED,
    ERROR,
    EXPAND_MORE_ROUNDED,
    FAVORITE,
    FAVORITE_BORDER,
    FORMAT_ALIGN_CENTER,
    FORMAT_ALIGN_LEFT,
    FORMAT_ALIGN_RIGHT,
    HOME,
    HORIZONTAL_RULE,
    HOURGLASS_EMPTY,
    IMAGE,
    INBOX,
    INFO,
    KEYBOARD_ARROW_DOWN,
    LABEL,
    LIGHT_MODE,
    LINEAR_SCALE,
    LIST,
    MAIL,
    MENU,
    MENU_OPEN,
    MESSAGE,
    MIC,
    MORE_HORIZ,
    MORE_VERT,
    NOTIFICATIONS,
    PALETTE,
    RADIO_BUTTON_CHECKED,
    REFRESH,
    REPORT,
    SAVE,
    SCHEDULE,
    SEARCH,
    SELECT_ALL,
    SEND,
    SHARE,
    SMART_BUTTON,
    SPACE_DASHBOARD,
    STAR,
    STAR_OUTLINE,
    SYSTEM_UPDATE,
    TAB,
    TAG,
    TEXT_FIELDS,
    TOGGLE_ON,
    TUNE,
    VERTICAL_ALIGN_BOTTOM,
    VERTICAL_SPLIT,
    VIDEOCAM,
    VIEW_CAROUSEL,
    VIEW_COLUMN,
    VIEW_SIDEBAR,
    VIEW_WEEK,
    VOLUME_UP,
    WEB_ASSET,
];
