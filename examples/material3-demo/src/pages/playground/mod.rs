//! One module per catalog entry: the playground body that entry opens.
//!
//! # The page contract
//!
//! Every module here exposes exactly one fn:
//!
//! ```ignore
//! pub fn page(entry: DemoEntry) -> AnyView<AppState>
//! ```
//!
//! taking the [`crate::catalog::DemoEntry`] that named it, which is the
//! signature every catalog row's `build` field holds. It returns the
//! playground **body only** — the host supplies
//! chrome (an app bar when the page was pushed, nothing when it sits in the
//! split layout's detail pane), so a page never builds its own.
//!
//! A page owns its knobs as a nested [`frust::Component`], not as fields on
//! [`crate::AppState`]: the component's own retained state is what survives
//! rebuilds, and it keeps 39 pages from touching one shared struct. A page
//! that needs to mount an overlay (dialog, sheet, menu) does it the way
//! `examples/shadcn-demo`'s overlays page does — a `NavigatorController` in
//! its own state, with the navigator as the page's outermost view — so the
//! modal is scoped to the page rather than to the gallery.
//!
//! `do_` is spelled with the trailing underscore because `do` is a Rust
//! keyword; the section's own labels stay the reference's `Do`.

pub mod do_;
pub mod find;
pub mod nav;
pub mod pick;
pub mod view;

#[cfg(test)]
mod tests {
    use crate::catalog::{self, DemoSection};

    /// Every catalog row builds the page it points at. Cheap here, invisible
    /// otherwise: a mis-wired `build` field only shows up on screen.
    #[test]
    fn every_catalog_entry_builds_its_playground() {
        for section in DemoSection::ALL {
            for entry in catalog::for_section(section) {
                let _view = (entry.build)(*entry);
            }
        }
    }
}
