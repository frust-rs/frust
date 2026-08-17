//! Deprecated compatibility shim: `ListView` moved to the framework's
//! baseline widget set (`frust::ListView`/`frust::ListViewWidget`/
//! `frust::list_view`) well before this catalog existed as its own crate.
//! Kept only so an existing `frust_material::list_view::…` call site still
//! resolves.
//!
//! Each item below is individually `#[deprecated]` rather than the module
//! itself, and each is a type alias / thin wrapper rather than a `pub use` —
//! `#[deprecated]` attached to a `use` re-export does not surface a warning
//! at the *use site* going through that re-export (a confirmed rustc
//! limitation, not a mistake here); a concrete item carrying its own
//! `#[deprecated]` does.

/// Deprecated: moved to [`frust::ListView`].
#[deprecated(note = "ListView moved to the baseline widget set — use `frust::ListView` instead")]
pub type ListView<State> = frust::ListView<State>;

/// Deprecated: moved to [`frust::ListViewWidget`].
#[deprecated(
    note = "ListView moved to the baseline widget set — use `frust::ListViewWidget` instead"
)]
pub type ListViewWidget = frust::ListViewWidget;

/// Deprecated: moved to [`frust::list_view`].
#[deprecated(note = "ListView moved to the baseline widget set — use `frust::list_view` instead")]
pub fn list_view<State: 'static>(
    item_count: usize,
    item_extent: f64,
    builder: impl Fn(usize) -> frust::authoring::AnyView<State> + 'static,
) -> frust::ListView<State> {
    frust::list_view(item_count, item_extent, builder)
}
