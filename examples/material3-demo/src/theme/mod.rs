//! The gallery's theme layer: the settings object every screen edits and the
//! theme it produces.
//!
//! [`ThemeSettings`] rides `provide_context` from the root component, the way
//! the reference's `ExampleThemeScope` rides an `InheritedNotifier` — that
//! type's own docs carry the model and how it reaches the shell.

mod settings;

pub use settings::{DemoFont, DemoTypeStyle, SEED_OPTIONS, ThemeSettings};
