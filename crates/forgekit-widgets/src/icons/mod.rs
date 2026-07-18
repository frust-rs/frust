//! Vendored Material Symbols starter set — GENERATED, do not hand-edit.
//!
//! Each entry is a [`IconSource`](crate::IconSource) carrying a Material Symbols
//! glyph as SVG path `d` data in a 24×24 design box. Pass one to
//! [`icon`](crate::icon) (or convert it via `IconData::from(..)`) to paint it.
//!
//! # Provenance & license
//!
//! Material Symbols is licensed Apache-2.0 (see
//! `LICENSE-material-symbols` alongside this module). Google publishes the icons
//! with no NOTICE file and no embedded per-file copyright, and attribution is
//! optional per Google's own guidance — so the path data is safe to vendor. The
//! starter glyphs below are authored in the generator's output format at
//! Material Symbols' 24×24 grid; a maintainer regenerates the full set from a
//! real Material Symbols SVG checkout with the command below.
//!
//! # Regenerating
//!
//! ```text
//! python3 scripts/gen_icons.py --src <material-symbols-svg-dir> \
//!     --out crates/forgekit-widgets/src/icons/mod.rs
//! ```
//!
//! The script reads each `<name>.svg`, extracts its single `<path d="...">`,
//! validates it parses, and re-emits this file (module header, the `pub const`
//! table, and the [`ALL`] slice). See `scripts/gen_icons.py`.

use crate::IconSource;

/// The design-box side length every entry below is authored against.
const D: f64 = 24.0;

/// Home / dashboard.
pub const HOME: IconSource = IconSource {
    d: "M10 20v-6h4v6h5v-8h3L12 3 4 12h3v8z",
    design: D,
};

/// Search / find.
pub const SEARCH: IconSource = IconSource {
    d: "M15.5 14h-.79l-.28-.27a6.5 6.5 0 1 0-.7.7l.27.28v.79l5 4.99L20.49 19zm-6 0A4.5 4.5 0 1 1 14 9.5 4.5 4.5 0 0 1 9.5 14z",
    design: D,
};

/// Notifications / activity bell.
pub const NOTIFICATIONS: IconSource = IconSource {
    d: "M12 22a2 2 0 0 0 2-2h-4a2 2 0 0 0 2 2zm6-6v-5c0-3.07-1.63-5.64-4.5-6.32V4a1.5 1.5 0 0 0-3 0v.68C7.64 5.36 6 7.92 6 11v5l-2 2v1h16v-1z",
    design: D,
};

/// Person / you.
pub const PERSON: IconSource = IconSource {
    d: "M12 12a5 5 0 1 0-5-5 5 5 0 0 0 5 5zm0 2c-3.33 0-10 1.67-10 5v3h20v-3c0-3.33-6.67-5-10-5z",
    design: D,
};

/// Tag / channel label.
pub const TAG: IconSource = IconSource {
    d: "M21.41 11.58l-9-9A2 2 0 0 0 11 2H4a2 2 0 0 0-2 2v7a2 2 0 0 0 .59 1.42l9 9a2 2 0 0 0 2.82 0l7-7a2 2 0 0 0 0-2.84zM5.5 7A1.5 1.5 0 1 1 7 5.5 1.5 1.5 0 0 1 5.5 7z",
    design: D,
};

/// Lock / private channel.
pub const LOCK: IconSource = IconSource {
    d: "M18 8h-1V6A5 5 0 0 0 7 6v2H6a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V10a2 2 0 0 0-2-2zm-6 9a2 2 0 1 1 2-2 2 2 0 0 1-2 2zm3-9H9V6a3 3 0 0 1 6 0z",
    design: D,
};

/// Send message.
pub const SEND: IconSource = IconSource {
    d: "M2 21l21-9L2 3v7l15 2-15 2z",
    design: D,
};

/// Add / plus.
pub const ADD: IconSource = IconSource {
    d: "M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6z",
    design: D,
};

/// Emoji / mood.
pub const MOOD: IconSource = IconSource {
    d: "M12 2a10 10 0 1 0 10 10A10 10 0 0 0 12 2zm-3.5 7a1.5 1.5 0 1 1-1.5 1.5A1.5 1.5 0 0 1 8.5 9zm7 0a1.5 1.5 0 1 1-1.5 1.5A1.5 1.5 0 0 1 15.5 9zM12 18a5.5 5.5 0 0 1-5-3h10a5.5 5.5 0 0 1-5 3z",
    design: D,
};

/// Attach file.
pub const ATTACH_FILE: IconSource = IconSource {
    d: "M16.5 6v11.5a4 4 0 0 1-8 0V5a2.5 2.5 0 0 1 5 0v10.5a1 1 0 0 1-2 0V6H10v9.5a2.5 2.5 0 0 0 5 0V5a4 4 0 0 0-8 0v12.5a5.5 5.5 0 0 0 11 0V6z",
    design: D,
};

/// Reply.
pub const REPLY: IconSource = IconSource {
    d: "M10 9V5l-7 7 7 7v-4.1c5 0 8.5 1.6 11 5.1-1-5-4-10-11-11z",
    design: D,
};

/// Thread / forum.
pub const FORUM: IconSource = IconSource {
    d: "M21 6h-2v9H6v2a1 1 0 0 0 1 1h11l4 4V7a1 1 0 0 0-1-1zm-4 6V3a1 1 0 0 0-1-1H3a1 1 0 0 0-1 1v14l4-4h10a1 1 0 0 0 1-1z",
    design: D,
};

/// Overflow menu (vertical).
pub const MORE_VERT: IconSource = IconSource {
    d: "M12 8a2 2 0 1 0-2-2 2 2 0 0 0 2 2zm0 2a2 2 0 1 0 2 2 2 2 0 0 0-2-2zm0 6a2 2 0 1 0 2 2 2 2 0 0 0-2-2z",
    design: D,
};

/// Overflow menu (horizontal).
pub const MORE_HORIZ: IconSource = IconSource {
    d: "M6 10a2 2 0 1 0 2 2 2 2 0 0 0-2-2zm12 0a2 2 0 1 0 2 2 2 2 0 0 0-2-2zm-6 0a2 2 0 1 0 2 2 2 2 0 0 0-2-2z",
    design: D,
};

/// Close / dismiss.
pub const CLOSE: IconSource = IconSource {
    d: "M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z",
    design: D,
};

/// Check / confirm.
pub const CHECK: IconSource = IconSource {
    d: "M9 16.17 4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z",
    design: D,
};

/// Back arrow.
pub const ARROW_BACK: IconSource = IconSource {
    d: "M20 11H7.83l5.59-5.59L12 4l-8 8 8 8 1.41-1.41L7.83 13H20z",
    design: D,
};

/// Chevron right / disclosure.
pub const CHEVRON_RIGHT: IconSource = IconSource {
    d: "M10 6L8.59 7.41 13.17 12l-4.58 4.59L10 18l6-6z",
    design: D,
};

/// Settings gear.
pub const SETTINGS: IconSource = IconSource {
    d: "M19.14 12.94a7.49 7.49 0 0 0 .05-1 7.49 7.49 0 0 0-.05-1l2.03-1.58a.5.5 0 0 0 .12-.61l-1.92-3.32a.5.5 0 0 0-.59-.22l-2.39.96a7 7 0 0 0-1.69-.98l-.36-2.54a.49.49 0 0 0-.5-.42h-3.84a.49.49 0 0 0-.5.42l-.36 2.54a7 7 0 0 0-1.69.98l-2.39-.96a.5.5 0 0 0-.59.22L2.7 8.87a.5.5 0 0 0 .12.61l2.03 1.58a7.49 7.49 0 0 0 0 2l-2.03 1.58a.5.5 0 0 0-.12.61l1.92 3.32a.5.5 0 0 0 .59.22l2.39-.96a7 7 0 0 0 1.69.98l.36 2.54a.49.49 0 0 0 .5.42h3.84a.49.49 0 0 0 .5-.42l.36-2.54a7 7 0 0 0 1.69-.98l2.39.96a.5.5 0 0 0 .59-.22l1.92-3.32a.5.5 0 0 0-.12-.61zM12 15.5a3.5 3.5 0 1 1 3.5-3.5 3.5 3.5 0 0 1-3.5 3.5z",
    design: D,
};

/// Edit / compose.
pub const EDIT: IconSource = IconSource {
    d: "M3 17.25V21h3.75L17.81 9.94l-3.75-3.75zM20.71 7.04a1 1 0 0 0 0-1.41l-2.34-2.34a1 1 0 0 0-1.41 0l-1.83 1.83 3.75 3.75z",
    design: D,
};

/// Delete / trash.
pub const DELETE: IconSource = IconSource {
    d: "M6 19a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2V7H6zM19 4h-3.5l-1-1h-5l-1 1H5v2h14z",
    design: D,
};

/// Archive.
pub const ARCHIVE: IconSource = IconSource {
    d: "M20.54 5.23l-1.39-1.68A1.45 1.45 0 0 0 18 3H6a1.45 1.45 0 0 0-1.11.55L3.46 5.23A2 2 0 0 0 3 6.5V19a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V6.5a2 2 0 0 0-.46-1.27zM12 17.5L6.5 12H10v-2h4v2h3.5zM5.12 5l.81-1h12l.94 1z",
    design: D,
};

/// Mute / volume off.
pub const VOLUME_OFF: IconSource = IconSource {
    d: "M16.5 12A4.5 4.5 0 0 0 14 7.97v2.21l2.45 2.45A4.5 4.5 0 0 0 16.5 12zM3 9v6h4l5 5V4L7 9zm16.5 3a7.45 7.45 0 0 1-.63 3.01l1.52 1.52A9.45 9.45 0 0 0 21 12z",
    design: D,
};

/// Pin.
pub const PUSH_PIN: IconSource = IconSource {
    d: "M16 9V4h1a1 1 0 0 0 0-2H7a1 1 0 0 0 0 2h1v5a3 3 0 0 1-3 3v2h5.97v6l1 1 1-1v-6H19v-2a3 3 0 0 1-3-3z",
    design: D,
};

/// Star (outline).
pub const STAR: IconSource = IconSource {
    d: "M22 9.24l-7.19-.62L12 2 9.19 8.63 2 9.24l5.46 4.73L5.82 21 12 17.27 18.18 21l-1.63-7.03zM12 15.4l-3.76 2.27 1-4.28-3.32-2.88 4.38-.38L12 6.1l1.71 4.04 4.38.38-3.32 2.88 1 4.28z",
    design: D,
};

/// Star (filled).
pub const STAR_FILLED: IconSource = IconSource {
    d: "M12 17.27 18.18 21l-1.64-7.03L22 9.24l-7.19-.61L12 2 9.19 8.63 2 9.24l5.46 4.73L5.82 21z",
    design: D,
};

/// Dark mode.
pub const DARK_MODE: IconSource = IconSource {
    d: "M12 3a9 9 0 1 0 9 9c0-.46-.04-.92-.1-1.36a5.39 5.39 0 0 1-4.4 2.26 5.5 5.5 0 0 1-5.5-5.5 5.39 5.39 0 0 1 2.26-4.4A9 9 0 0 0 12 3z",
    design: D,
};

/// Light mode.
pub const LIGHT_MODE: IconSource = IconSource {
    d: "M12 7a5 5 0 1 0 5 5 5 5 0 0 0-5-5zm0-5a1 1 0 0 0-1 1v1a1 1 0 0 0 2 0V3a1 1 0 0 0-1-1zm0 18a1 1 0 0 0-1 1v1a1 1 0 0 0 2 0v-1a1 1 0 0 0-1-1zM5.64 5.64a1 1 0 0 0-1.41 0 1 1 0 0 0 0 1.41l.7.71a1 1 0 0 0 1.42-1.42zM18.36 18.36l-.71-.71a1 1 0 0 0-1.41 1.41l.7.71a1 1 0 0 0 1.42-1.41zM3 11H2a1 1 0 0 0 0 2h1a1 1 0 0 0 0-2zm19 0h-1a1 1 0 0 0 0 2h1a1 1 0 0 0 0-2zM6.34 17.66l-.7.7a1 1 0 1 0 1.41 1.42l.71-.71a1 1 0 0 0-1.42-1.41zM18.36 5.64a1 1 0 0 0-1.42 0l-.7.71a1 1 0 0 0 1.41 1.41l.71-.7a1 1 0 0 0 0-1.42z",
    design: D,
};

/// Palette / theme.
pub const PALETTE: IconSource = IconSource {
    d: "M12 3a9 9 0 0 0 0 18 1.5 1.5 0 0 0 1.15-2.47.9.9 0 0 1-.22-.59 1 1 0 0 1 1-1H15a6 6 0 0 0 6-6c0-4.42-4.03-8-9-8zm-5.5 9a1.5 1.5 0 1 1 1.5-1.5A1.5 1.5 0 0 1 6.5 12zm3-4a1.5 1.5 0 1 1 1.5-1.5A1.5 1.5 0 0 1 9.5 8zm5 0a1.5 1.5 0 1 1 1.5-1.5A1.5 1.5 0 0 1 14.5 8zm3 4a1.5 1.5 0 1 1 1.5-1.5 1.5 1.5 0 0 1-1.5 1.5z",
    design: D,
};

/// Text size.
pub const FORMAT_SIZE: IconSource = IconSource {
    d: "M9 4v3h5v12h3V7h5V4zm-6 8h3v7h3v-7h3V9H3z",
    design: D,
};

/// Log out / sign out.
pub const LOGOUT: IconSource = IconSource {
    d: "M17 8l-1.41 1.41L17.17 11H9v2h8.17l-1.58 1.58L17 16l4-4zM5 5h7V3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h7v-2H5z",
    design: D,
};

/// Group / team.
pub const GROUP: IconSource = IconSource {
    d: "M16 11a3 3 0 1 0-3-3 3 3 0 0 0 3 3zm-8 0a3 3 0 1 0-3-3 3 3 0 0 0 3 3zm0 2c-2.33 0-7 1.17-7 3.5V19h8v-2.5c0-.85.33-2.34 2.37-3.47A12.4 12.4 0 0 0 8 13zm8 0c-.29 0-.62 0-.97.03A4.6 4.6 0 0 1 18 16.5V19h5v-2.5c0-2.33-4.67-3.5-7-3.5z",
    design: D,
};

/// Image / picture.
pub const IMAGE: IconSource = IconSource {
    d: "M21 19V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2zM8.5 13.5l2.5 3 3.5-4.5 4.5 6H5z",
    design: D,
};

/// File / description.
pub const DESCRIPTION: IconSource = IconSource {
    d: "M6 2a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8l-6-6zm7 7V3.5L18.5 9zM8 13h8v2H8zm0 4h8v2H8z",
    design: D,
};

/// Link / attachment URL.
pub const LINK: IconSource = IconSource {
    d: "M3.9 12a3.1 3.1 0 0 1 3.1-3.1h4V7H7a5 5 0 0 0 0 10h4v-1.9H7A3.1 3.1 0 0 1 3.9 12zM8 13h8v-2H8zm9-6h-4v1.9h4a3.1 3.1 0 0 1 0 6.2h-4V17h4a5 5 0 0 0 0-10z",
    design: D,
};

/// Refresh / retry.
pub const REFRESH: IconSource = IconSource {
    d: "M17.65 6.35A8 8 0 1 0 19.73 14h-2.08a6 6 0 1 1-1.42-6.22L13 11h7V4z",
    design: D,
};

/// Microphone / voice.
pub const MIC: IconSource = IconSource {
    d: "M12 14a3 3 0 0 0 3-3V5a3 3 0 0 0-6 0v6a3 3 0 0 0 3 3zm5-3a5 5 0 0 1-10 0H5a7 7 0 0 0 6 6.92V21h2v-3.08A7 7 0 0 0 19 11z",
    design: D,
};

/// Video camera.
pub const VIDEOCAM: IconSource = IconSource {
    d: "M17 10.5V7a1 1 0 0 0-1-1H4a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-3.5l4 4v-11z",
    design: D,
};

/// Phone / call.
pub const CALL: IconSource = IconSource {
    d: "M6.62 10.79a15.15 15.15 0 0 0 6.59 6.59l2.2-2.2a1 1 0 0 1 1.02-.24 11.36 11.36 0 0 0 3.57.57 1 1 0 0 1 1 1V20a1 1 0 0 1-1 1A17 17 0 0 1 3 4a1 1 0 0 1 1-1h3.5a1 1 0 0 1 1 1 11.36 11.36 0 0 0 .57 3.57 1 1 0 0 1-.25 1.02z",
    design: D,
};

/// Clock / schedule.
pub const SCHEDULE: IconSource = IconSource {
    d: "M12 2a10 10 0 1 0 10 10A10 10 0 0 0 12 2zm0 18a8 8 0 1 1 8-8 8 8 0 0 1-8 8zm.5-13H11v6l5.25 3.15.75-1.23-4.5-2.67z",
    design: D,
};

/// Done all / read receipts.
pub const DONE_ALL: IconSource = IconSource {
    d: "M18 7l-1.41-1.41-6.34 6.34 1.41 1.41zm4.24-1.41L11.66 16.17 7.48 12l-1.41 1.41L11.66 19l12-12zM.41 13.41L6 19l1.41-1.41L1.83 12z",
    design: D,
};

/// Every generated [`IconSource`] in this module, for exhaustive iteration
/// (e.g. a parse-validation test, or a picker gallery).
pub const ALL: &[IconSource] = &[
    HOME,
    SEARCH,
    NOTIFICATIONS,
    PERSON,
    TAG,
    LOCK,
    SEND,
    ADD,
    MOOD,
    ATTACH_FILE,
    REPLY,
    FORUM,
    MORE_VERT,
    MORE_HORIZ,
    CLOSE,
    CHECK,
    ARROW_BACK,
    CHEVRON_RIGHT,
    SETTINGS,
    EDIT,
    DELETE,
    ARCHIVE,
    VOLUME_OFF,
    PUSH_PIN,
    STAR,
    STAR_FILLED,
    DARK_MODE,
    LIGHT_MODE,
    PALETTE,
    FORMAT_SIZE,
    LOGOUT,
    GROUP,
    IMAGE,
    DESCRIPTION,
    LINK,
    REFRESH,
    MIC,
    VIDEOCAM,
    CALL,
    SCHEDULE,
    DONE_ALL,
];
