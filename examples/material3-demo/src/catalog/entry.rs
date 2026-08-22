//! One catalog row: a component's list metadata plus the playground it opens.

use frust::{AnyView, IconSource};

use crate::AppState;
use crate::catalog::DemoSection;

/// The one signature every playground page in this gallery is built through.
///
/// A plain `fn` pointer (not a boxed closure) so the catalog stays a `const`
/// table, and it takes the whole [`DemoEntry`] so a page can title/label
/// itself without the catalog's metadata being duplicated into 39 files.
/// Pages retain their own knob state inside a nested
/// [`frust::Component`] — none of them touch [`AppState`],
/// which is why one signature covers all 39.
pub type PlaygroundBuilder = fn(DemoEntry) -> AnyView<AppState>;

/// One component row in a section list, and the playground it opens.
#[derive(Clone, Copy)]
pub struct DemoEntry {
    /// Stable id: the selection key, and the `:id` in `/playground/:id`.
    pub id: &'static str,
    /// List headline.
    pub title: &'static str,
    /// List supporting text.
    pub subtitle: &'static str,
    /// Leading icon.
    pub icon: IconSource,
    /// Parent gallery section. Redundant at runtime (`for_section` serves
    /// per-section const tables) but read by the misfiling invariant test —
    /// every entry must sit in the table matching this field.
    #[allow(dead_code)]
    pub section: DemoSection,
    /// Builds the playground body (no chrome — the host supplies that).
    pub build: PlaygroundBuilder,
}

impl DemoEntry {
    /// This entry's playground route on the gallery's outer navigator — the
    /// page a narrow window pushes when the row is tapped.
    pub fn route(&self) -> String {
        format!("/playground/{}", self.id)
    }
}
