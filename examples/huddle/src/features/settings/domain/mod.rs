//! `settings` domain — the design-language + brightness selection spine, the
//! accent/notification vocabulary, and the [`SetTheme`] use case (huddle
//! clean-architecture refactor, task 05). No repository trait: nothing here
//! reads the shared store (verified — `grep -rn "mock::\|data::store"
//! domain/ presentation/` finds no hits), so this feature has no `data/`
//! layer.

pub mod accent;
pub mod models;
pub mod notifications;
pub mod use_cases;

pub use accent::AccentChoice;
pub use models::{
    BrightnessChoice, DesignChoice, SetThemeParams, TYPE_SCALE_DEFAULT, TYPE_SCALE_MAX,
    TYPE_SCALE_MIN, ThemeDecision, compose, scale_type_scale, slider_to_type_factor,
    type_factor_to_slider,
};
pub use notifications::NotifFrequency;
pub use use_cases::SetTheme;
