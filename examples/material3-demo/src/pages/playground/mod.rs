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
//! # Every navigator-handed builder — root *or* pushed — must close over live signals
//!
//! A [`frust::navigator`] retains whatever `Fn() -> View` it is handed and
//! re-invokes it on **every later rebuild**, not just when that specific
//! page's own state changes (`frust_widgets::nav`'s reconcile loop). This
//! binds every such builder a page hands a navigator, at any depth — the
//! **root** page builder `navigator(&nav, ..)` itself takes, and any
//! **pushed** page builder a `show_*`/`push_*` call inside that root page
//! hands the *same* navigator (a dialog, a bottom sheet, a menu). A closure
//! that captures a plain value — cloned or moved in *before* the closure
//! runs, frozen at that instant — silently stops reflecting later writes:
//! the value looks live at first paint and then never changes again, even
//! though the app state backing it keeps updating. `pick::date_pickers`,
//! `pick::time_pickers`, and `do_::split_button` each shipped this bug at
//! the pushed-builder depth and record the specific fix in their own module
//! docs.
//!
//! The rule: **every `Fn() -> View` handed to a navigator, root or pushed,
//! must capture signal *handles* (`RwSignal<T>`/`OverlayAnchor`/
//! `NavigatorController` — the `Clone` types wrapping shared interior-mutable
//! state) and re-read them with `.get()` (or an equivalent live read) inside
//! the closure body, never before it.** A plain `T` moved into the closure
//! is the defect, regardless of how many frames it takes to notice — see
//! `docs/CODE_STANDARDS.md`'s State & Reactivity conventions for the
//! sanctioned "something outside the component's own `build` observes this
//! write" signal-handle pattern this rule is an instance of.
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
