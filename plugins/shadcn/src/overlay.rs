//! The shared overlay hosting seam the catalog's overlay families will build on:
//! a full-area anchored-overlay layer for the non-modal panels (popover,
//! tooltip, hover-card, dropdown/context menu, select, combobox) and the
//! transparent-page + own-scrim modal pattern for the rest (dialog,
//! alert-dialog, sheet, drawer, command).
//!
//! **Empty by design at this stage.** The two hosting patterns are public
//! framework surface already — `frust`'s navigator re-exports
//! (`push_transparent_for_result`, the `frust_material::dialog` precedent) for a
//! modal, and the absolute-window-space `PaintCtx::origin` contract plus a
//! full-area `Stack` layer (the `frust_material::fab_menu` precedent) for an
//! anchored panel — so this module exists to hold the *one* shared host both
//! families reach for, not to re-derive either pattern per component. It lands
//! with the overlay batch; the component modules that consume it are stubs for
//! now too.
