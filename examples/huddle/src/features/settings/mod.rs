//! `settings` feature — the design-language + brightness selection spine,
//! notification preferences, and the four settings screens (huddle
//! clean-architecture refactor, task 05).
//!
//! No `data/` layer: nothing in this feature reads the shared store
//! (verified — `grep -rn "mock::\|data::store" domain/ presentation/`
//! finds no hits), so [`SetTheme`]/[`NotificationsController`] stay exactly
//! as infallible as before the refactor — see [`domain`]'s module docs.

pub mod domain;
pub mod presentation;

pub use domain::{
    AccentChoice, BrightnessChoice, DesignChoice, NotifFrequency, SetTheme, TYPE_SCALE_DEFAULT,
    TYPE_SCALE_MAX, TYPE_SCALE_MIN, ThemeDecision, compose, slider_to_type_factor,
    type_factor_to_slider,
};
pub use presentation::{NotificationsController, SettingsController};
