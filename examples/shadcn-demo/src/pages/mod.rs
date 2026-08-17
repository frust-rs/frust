//! One module per gallery page. Every page's `State` lives in
//! [`crate::AppState`]; `page(&State) -> impl View<AppState>` reads the
//! current values and every interactive callback writes back through the
//! full `AppState` path (`s.<page>.<field>`).

pub mod anchored;
pub mod controls;
pub mod inputs_table;
pub mod overlays;
pub mod primitives;
pub mod theming;
