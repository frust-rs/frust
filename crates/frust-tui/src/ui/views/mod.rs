//! Screen views: each renders `&AppState` into a frame area and (where
//! interactive) registers mouse regions. Views never mutate the engine.

pub mod too_small;
pub mod welcome;
pub mod workbench;
